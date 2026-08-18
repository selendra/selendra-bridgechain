// SPDX-License-Identifier: MIT
pragma solidity 0.8.24;

import {IERC20} from "@openzeppelin/contracts/token/ERC20/IERC20.sol";
import {SafeERC20} from "@openzeppelin/contracts/token/ERC20/utils/SafeERC20.sol";
import {ReentrancyGuard} from "@openzeppelin/contracts/utils/ReentrancyGuard.sol";
import {Gate} from "./Gate.sol";
import {SwapPool} from "./SwapPool.sol";

/// @title SwapRouter
/// @notice Composes the same-chain {SwapPool} with the cross-chain {Gate} to turn
///         "swap TokenX on chain A into TokenY on chain B" into two atomic legs,
///         WITHOUT changing either underlying contract. One router is deployed per
///         chain and pointed at that chain's Gate + SwapPool.
///
/// @dev    The stablecoin is the cross-chain carrier: only the stable needs bridge
///         liquidity on every chain (the pricing-hub payoff). A transfer is:
///
///           on chain A  swapAndBridge(): TokenX --SwapPool--> stable
///                                        stable --Gate.send--> chainB
///                                        intent (TokenY, receiver, minOut) rides
///                                        in autoParams.data, which the Gate binds
///                                        into the submissionId.
///           off-chain: validators sign the Sent (unchanged); a keeper claims.
///           on chain B  Gate.claim():     releases the stable to THIS router
///                       finalize():       stable --SwapPool--> TokenY -> receiver
///
///         The destination leg is TRUSTLESS and needs no callback from the Gate:
///         `finalize` proves delivery by checking `Gate.executed[submissionId]`.
///         Because `amount` and the intent (in autoParams.data) are both committed
///         inside that id — signed by the validators — the router can trust that
///         exactly `amount` of the stable was delivered to it for this intent. A
///         per-id `finalized` guard makes it idempotent, and any failure of the
///         destination swap (e.g. output would exceed the pool lock) falls back to
///         delivering the stable itself, so funds never strand.
contract SwapRouter is ReentrancyGuard {
    using SafeERC20 for IERC20;

    /// @notice the Gate this router bridges through (source send / dest claim).
    Gate public immutable gate;
    /// @notice the local SwapPool used for both swap legs.
    SwapPool public immutable pool;
    /// @notice the cross-chain carrier asset (must be listed on both pools).
    address public immutable stable;

    /// @notice Gas that must remain when the destination swap is attempted.
    ///
    /// @dev    GRIEFING GUARD. `_deliver` wraps `pool.swap` in try/catch so a
    ///         genuinely impossible swap (output over the pool lock, token
    ///         unlisted, slippage) falls back to delivering the stable instead of
    ///         stranding the funds. But `finalize` is PERMISSIONLESS and
    ///         `finalized[submissionId]` is set before the swap is attempted, so
    ///         without a floor here anyone could call it with just enough gas that
    ///         `pool.swap` runs out inside the call — the 63/64 rule leaves this
    ///         frame alive to catch it — and the user receives the carrier stable
    ///         instead of the token they asked for. The idempotency guard means
    ///         there is no second attempt.
    ///
    ///         So the fallback must be reachable only when the swap genuinely
    ///         cannot succeed, never merely because the caller was stingy. Sized
    ///         well above a SwapPool swap (two SafeERC20 transfers, a mulDiv and
    ///         two storage writes measure ~90k) with room for a cold-slot worst
    ///         case; a real relayer forwards far more.
    uint256 public constant MIN_DELIVER_GAS = 250_000;

    // --- governance (mirrors Gate.sol two-step ownership) ---
    address public owner;
    address public pendingOwner;

    /// @notice the trusted peer router on each destination chain; the bridged
    ///         stable is sent to this address so its `finalize` can complete the
    ///         second leg. Set by the owner per corridor.
    mapping(uint256 chainIdTo => bytes remoteRouter) public remoteRouter;

    /// @notice per-submission idempotency guard for the destination leg.
    mapping(bytes32 submissionId => bool) public finalized;

    // --- events ---
    event SwapBridged(
        bytes32 indexed submissionId,
        address indexed sender,
        address tokenIn,
        uint256 amountIn,
        uint256 stableOut,
        uint256 chainIdTo,
        address finalToken,
        address finalReceiver
    );
    event Finalized(
        bytes32 indexed submissionId,
        address indexed finalReceiver,
        address finalToken,
        uint256 stableIn,
        uint256 amountOut,
        bool swapped
    );
    /// @dev the destination swap could not complete (e.g. output over the pool
    ///      lock, or token unlisted); the stable was delivered instead.
    event FinalizeFallback(bytes32 indexed submissionId, address indexed finalReceiver, uint256 stableAmount);
    event RemoteRouterSet(uint256 indexed chainIdTo, bytes remoteRouter);
    event OwnershipTransferStarted(address indexed previousOwner, address indexed newOwner);
    event OwnershipTransferred(address indexed previousOwner, address indexed newOwner);

    // --- errors ---
    error NotOwner();
    error ZeroAddress();
    error ZeroAmount();
    error RouteNotConfigured(uint256 chainIdTo);
    error NotDelivered(bytes32 submissionId);
    error AlreadyFinalized(bytes32 submissionId);
    error NotForThisRouter();
    error UnexpectedAsset();
    error BadReceiver();
    /// @dev the caller did not forward enough gas for the destination swap to be
    ///      attempted honestly. See {MIN_DELIVER_GAS}.
    error InsufficientGas(uint256 have, uint256 need);

    modifier onlyOwner() {
        if (msg.sender != owner) revert NotOwner();
        _;
    }

    constructor(Gate gate_, SwapPool pool_) {
        if (address(gate_) == address(0) || address(pool_) == address(0)) revert ZeroAddress();
        gate = gate_;
        pool = pool_;
        stable = pool_.stable();
        owner = msg.sender;
        emit OwnershipTransferred(address(0), msg.sender);
    }

    // ---------------------------------------------------------------------
    // Governance
    // ---------------------------------------------------------------------

    function transferOwnership(address newOwner) external onlyOwner {
        if (newOwner == address(0)) revert ZeroAddress();
        pendingOwner = newOwner;
        emit OwnershipTransferStarted(owner, newOwner);
    }

    function acceptOwnership() external {
        if (msg.sender != pendingOwner) revert NotOwner();
        emit OwnershipTransferred(owner, pendingOwner);
        owner = pendingOwner;
        pendingOwner = address(0);
    }

    /// @notice Register the peer router that receives the bridged stable on
    ///         `chainIdTo`. `router` is the raw receiver bytes the destination
    ///         Gate will decode (20 bytes for an EVM router). Pass empty to clear.
    function setRemoteRouter(uint256 chainIdTo, bytes calldata router) external onlyOwner {
        if (router.length != 0 && router.length != 20 && router.length != 32) revert BadReceiver();
        remoteRouter[chainIdTo] = router;
        emit RemoteRouterSet(chainIdTo, router);
    }

    // ---------------------------------------------------------------------
    // Source leg: swap local -> stable, then bridge with the swap intent
    // ---------------------------------------------------------------------

    /// @notice Swap `amountIn` of `tokenIn` into the stable locally, then bridge
    ///         the stable to `chainIdTo`, carrying the intent to swap it into
    ///         `finalToken` for `finalReceiver` on arrival.
    /// @param tokenIn        the local token to sell (may be the stable itself)
    /// @param amountIn       amount of `tokenIn` to pull from the caller
    /// @param minStableOut   slippage floor for the LOCAL tokenIn->stable swap
    /// @param chainIdTo      destination chain id (must have a remoteRouter set)
    /// @param finalToken     the token to receive on the destination chain
    /// @param finalReceiver  the EVM recipient on the destination chain
    /// @param finalMinOut    slippage floor for the DESTINATION stable->finalToken swap
    function swapAndBridge(
        address tokenIn,
        uint256 amountIn,
        uint256 minStableOut,
        uint256 chainIdTo,
        address finalToken,
        address finalReceiver,
        uint256 finalMinOut
    ) external nonReentrant returns (bytes32 submissionId) {
        if (amountIn == 0) revert ZeroAmount();
        if (finalToken == address(0) || finalReceiver == address(0)) revert ZeroAddress();

        bytes memory receiver = remoteRouter[chainIdTo];
        if (receiver.length == 0) revert RouteNotConfigured(chainIdTo);

        // Leg 1: acquire the stable to bridge. If the caller is already paying in
        // the stable, skip the swap and forward it directly.
        uint256 stableOut;
        if (tokenIn == stable) {
            uint256 before = IERC20(stable).balanceOf(address(this));
            IERC20(stable).safeTransferFrom(msg.sender, address(this), amountIn);
            stableOut = IERC20(stable).balanceOf(address(this)) - before;
        } else {
            IERC20(tokenIn).safeTransferFrom(msg.sender, address(this), amountIn);
            IERC20(tokenIn).forceApprove(address(pool), amountIn);
            stableOut = pool.swap(tokenIn, stable, amountIn, minStableOut, address(this));
        }
        if (stableOut == 0) revert ZeroAmount();

        // The destination intent rides in autoParams.data, which the Gate binds
        // into the submissionId (so the validators sign over it). fallbackAddress
        // mirrors finalReceiver for parity with the Gate's AutoParams shape.
        Gate.AutoParamsTo memory ap = Gate.AutoParamsTo({
            executionFee: 0,
            flags: 0,
            fallbackAddress: abi.encodePacked(finalReceiver),
            data: abi.encode(finalToken, finalReceiver, finalMinOut)
        });

        // Leg 2: bridge the stable to the peer router on the destination chain.
        IERC20(stable).forceApprove(address(gate), stableOut);
        submissionId = gate.send(stable, stableOut, chainIdTo, receiver, abi.encode(ap));

        emit SwapBridged(
            submissionId, msg.sender, tokenIn, amountIn, stableOut, chainIdTo, finalToken, finalReceiver
        );
    }

    // ---------------------------------------------------------------------
    // Destination leg: prove delivery, then swap stable -> finalToken
    // ---------------------------------------------------------------------

    /// @notice Complete the destination swap for a bridge transfer that has
    ///         already been `claim`ed into this router. Permissionless: delivery
    ///         is proven cryptographically via `Gate.executed[submissionId]`, and
    ///         the swap intent is read back out of the same signed `autoParams`.
    ///         The bridge fields are exactly those from the source `Sent` event.
    function finalize(
        bytes32 debridgeId,
        uint256 amount,
        uint256 chainIdFrom,
        uint256 nonce,
        bytes calldata receiver,
        bytes calldata autoParams,
        bytes calldata nativeSender
    ) external nonReentrant returns (bytes32 submissionId) {
        submissionId = gate.computeSubmissionId(
            debridgeId, amount, chainIdFrom, block.chainid, nonce, receiver, autoParams, nativeSender
        );
        // Delivery proof: the Gate only sets this after verifying the validator
        // threshold, and the amount + intent are bound into the id it signed.
        //
        // `executed` alone is NOT delivery: `Gate.cancel` also sets it, to burn a
        // stranded transfer so it can be refunded on the source chain. In that
        // case no stable ever reached this router, so settling would pay the
        // receiver out of another transfer's in-flight liquidity — while the
        // source chain separately refunds the original sender. Both legs must be
        // checked together.
        if (!gate.executed(submissionId) || gate.cancelled(submissionId)) {
            revert NotDelivered(submissionId);
        }
        _settle(submissionId, debridgeId, amount, receiver, autoParams);
    }

    /// @notice Convenience wrapper: `claim` the bridge transfer into this router
    ///         and complete the destination swap in one transaction.
    function claimAndFinalize(
        bytes32 debridgeId,
        uint256 amount,
        uint256 chainIdFrom,
        uint256 nonce,
        bytes calldata receiver,
        bytes calldata autoParams,
        bytes calldata nativeSender,
        bytes[] calldata signatures
    ) external nonReentrant returns (bytes32 submissionId) {
        // Releases `amount` of the stable to `receiver` (this router) and sets
        // executed[submissionId]. Reverts if it was already claimed. `claim`
        // itself returns the submissionId, so there's no need to recompute it.
        submissionId =
            gate.claim(debridgeId, amount, chainIdFrom, nonce, receiver, autoParams, nativeSender, signatures);
        _settle(submissionId, debridgeId, amount, receiver, autoParams);
    }

    /// @dev Shared post-delivery tail for `finalize`/`claimAndFinalize`: confirm
    ///      the stable was released to this router for an asset it knows how to
    ///      route, mark idempotent, decode the swap intent, and deliver.
    function _settle(
        bytes32 submissionId,
        bytes32 debridgeId,
        uint256 amount,
        bytes calldata receiver,
        bytes calldata autoParams
    ) internal {
        if (finalized[submissionId]) revert AlreadyFinalized(submissionId);
        // The claim released the stable to `receiver`; it must be this router,
        // and the delivered asset must be the stable we know how to route.
        if (_toAddress(receiver) != address(this)) revert NotForThisRouter();
        if (gate.tokenOf(debridgeId) != stable) revert UnexpectedAsset();

        finalized[submissionId] = true; // effects before interactions (idempotent)

        (address finalToken, address finalReceiver, uint256 finalMinOut) =
            _decodeIntent(autoParams);
        if (finalReceiver == address(0)) revert ZeroAddress();

        _deliver(submissionId, amount, finalToken, finalReceiver, finalMinOut);
    }

    // ---------------------------------------------------------------------
    // Admin rescue
    // ---------------------------------------------------------------------

    /// @notice Sweep tokens stranded at the router (e.g. a transfer bridged here
    ///         with a malformed intent that can never `finalize`). The router is
    ///         only ever a transient custodian, so any resting balance is either
    ///         dust or stuck funds — same trust model as the Gate/pool owner.
    function rescue(address token, uint256 amount, address to) external onlyOwner {
        if (to == address(0)) revert ZeroAddress();
        IERC20(token).safeTransfer(to, amount);
    }

    // ---------------------------------------------------------------------
    // Internal
    // ---------------------------------------------------------------------

    /// @dev Swap the delivered stable into `finalToken` for `finalReceiver`. If
    ///      the swap cannot complete (output over the pool lock, token unlisted,
    ///      slippage), deliver the stable itself so the funds are never stranded.
    function _deliver(
        bytes32 submissionId,
        uint256 amount,
        address finalToken,
        address finalReceiver,
        uint256 finalMinOut
    ) internal {
        // Degenerate intent: the caller wanted the stable itself on this chain.
        if (finalToken == stable) {
            IERC20(stable).safeTransfer(finalReceiver, amount);
            emit Finalized(submissionId, finalReceiver, finalToken, amount, amount, false);
            return;
        }

        // Refuse to *attempt* the swap without enough gas to complete it, so the
        // catch below can only ever mean "this swap is impossible", never "the
        // caller starved it". Checked here rather than at function entry so it
        // covers exactly the call it protects.
        if (gasleft() < MIN_DELIVER_GAS) revert InsufficientGas(gasleft(), MIN_DELIVER_GAS);

        IERC20(stable).forceApprove(address(pool), amount);
        try pool.swap(stable, finalToken, amount, finalMinOut, finalReceiver) returns (uint256 out) {
            emit Finalized(submissionId, finalReceiver, finalToken, amount, out, true);
        } catch {
            // Roll back the unused allowance and refund the stable to the user.
            IERC20(stable).forceApprove(address(pool), 0);
            IERC20(stable).safeTransfer(finalReceiver, amount);
            emit FinalizeFallback(submissionId, finalReceiver, amount);
        }
    }

    /// @dev Read (finalToken, finalReceiver, finalMinOut) back out of the signed
    ///      Gate.AutoParamsTo.data. Reverts if the payload is malformed.
    function _decodeIntent(bytes calldata autoParams)
        internal
        pure
        returns (address finalToken, address finalReceiver, uint256 finalMinOut)
    {
        Gate.AutoParamsTo memory ap = abi.decode(autoParams, (Gate.AutoParamsTo));
        (finalToken, finalReceiver, finalMinOut) = abi.decode(ap.data, (address, address, uint256));
    }

    /// @dev Decode `receiver` as an EVM address, exactly as the Gate does — and
    ///      exactly as strictly: 20 bytes and no other width. `_settle` compares
    ///      the result against `address(this)` to prove the transfer was routed
    ///      here, so a looser decode here than in the Gate would let a padded
    ///      receiver satisfy that check on a width the Gate itself refuses. Keep
    ///      the two in lockstep.
    function _toAddress(bytes calldata receiver) internal pure returns (address addr) {
        if (receiver.length != 20) revert BadReceiver();
        addr = address(bytes20(receiver[0:20]));
    }
}
