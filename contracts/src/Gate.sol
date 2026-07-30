// SPDX-License-Identifier: MIT
pragma solidity 0.8.24;

import {IERC20} from "@openzeppelin/contracts/token/ERC20/IERC20.sol";
import {SafeERC20} from "@openzeppelin/contracts/token/ERC20/utils/SafeERC20.sol";
import {ECDSA} from "@openzeppelin/contracts/utils/cryptography/ECDSA.sol";
import {MessageHashUtils} from "@openzeppelin/contracts/utils/cryptography/MessageHashUtils.sol";
import {BridgeHash} from "./BridgeHash.sol";

/// @title Gate
/// @notice External-validator bridge gate, modeled on deBridge's DeBridgeGate.
///         Deployed on every supported chain. `send()` locks an ERC-20 and emits
///         a `Sent` event; `claim()` verifies a threshold of validator signatures
///         and releases funds exactly once (replay-safe).
/// @dev    EVM <-> EVM, lock/unlock model: the target gate holds pre-funded
///         liquidity of the local token registered for a debridgeId.
contract Gate {
    using SafeERC20 for IERC20;

    /// @dev To-side execution payload, abi.encode'd into `send`/`claim` autoParams.
    struct AutoParamsTo {
        uint256 executionFee;
        uint256 flags;
        bytes fallbackAddress;
        bytes data;
    }

    // --- validator set / governance ---
    address public owner;
    address public pendingOwner;
    mapping(address => bool) public isValidator;
    uint256 public validatorCount;
    uint256 public threshold;

    // --- emergency circuit breaker ---
    /// @dev when true, `send` and `claim` are halted (incident response)
    bool public paused;
    /// @dev may trip the breaker (fast incident response) but cannot un-pause;
    ///      only `owner` can resume. address(0) until the owner appoints one.
    address public guardian;

    // --- source-side state ---
    /// @dev per-target-chain monotonic nonce
    mapping(uint256 chainIdTo => uint256) public nonceTo;
    /// @dev who locked the funds for each submissionId this gate emitted.
    ///
    ///      Two jobs, both essential to `refund`:
    ///        1. **origin proof** — a nonzero entry is the only evidence that this
    ///           gate really sent `submissionId`. Without it a validator quorum
    ///           could authorise a refund for a transfer that never happened here.
    ///        2. **refund recipient** — `nativeSender` is only folded into the
    ///           submissionId when `autoParams` is non-empty, so for a plain
    ///           transfer it is NOT bound by the hash and a caller could name any
    ///           address. Storage is authoritative; the calldata is not trusted.
    ///
    ///      Cleared on `refund` (and left set otherwise, so it doubles as the
    ///      "still refundable" flag).
    mapping(bytes32 submissionId => address sender) public sentBy;
    /// @dev source-side replay guard: a submissionId may only be refunded once
    mapping(bytes32 submissionId => bool) public refunded;

    // --- target-side state ---
    /// @dev replay guard: a submissionId may only ever be executed once.
    ///      Set by `claim` (funds released) AND by `cancel` (funds permanently
    ///      NOT released) — check `cancelled` to tell the two apart.
    mapping(bytes32 submissionId => bool) public executed;
    /// @dev target-side: this submissionId was burned by `cancel`, not claimed.
    ///      Consumers that treat `executed` as "delivered" (e.g. SwapRouter's
    ///      `finalize`) MUST also check this, or they will act on a delivery that
    ///      never happened.
    mapping(bytes32 submissionId => bool) public cancelled;
    /// @dev asset registry: which local ERC-20 backs a given debridgeId on THIS chain
    mapping(bytes32 debridgeId => address localToken) public tokenOf;

    /// @param token the ERC-20 locked on THIS chain. Not part of the submissionId
    ///        (which commits to `debridgeId`, a one-way hash of it), so it is
    ///        emitted explicitly — the refund relayer needs the concrete address
    ///        to build `refund()`, and keccak is not invertible.
    event Sent(
        bytes32 indexed submissionId,
        bytes32 indexed debridgeId,
        uint256 amount,
        uint256 chainIdFrom,
        uint256 chainIdTo,
        bytes receiver,
        uint256 nonce,
        bytes autoParams,
        bytes nativeSender,
        address token
    );

    event Claimed(
        bytes32 indexed submissionId,
        bytes32 indexed debridgeId,
        address indexed receiver,
        uint256 amount
    );

    /// @notice Emitted on the DESTINATION chain when a transfer is burned so it
    ///         can never be claimed — the precondition for a source-side refund.
    event Cancelled(
        bytes32 indexed submissionId,
        bytes32 indexed debridgeId,
        uint256 chainIdFrom,
        uint256 nonce
    );

    /// @notice Emitted on the SOURCE chain when locked funds are returned.
    event Refunded(
        bytes32 indexed submissionId,
        bytes32 indexed debridgeId,
        address indexed sender,
        uint256 amount
    );

    // --- governance events (auditability) ---
    event OwnershipTransferStarted(address indexed previousOwner, address indexed newOwner);
    event OwnershipTransferred(address indexed previousOwner, address indexed newOwner);
    event ValidatorSet(address indexed validator, bool active);
    event ThresholdSet(uint256 threshold);
    event LocalTokenSet(bytes32 indexed debridgeId, address indexed localToken);
    event GuardianSet(address indexed guardian);
    event Paused(address indexed account);
    event Unpaused(address indexed account);

    error NotOwner();
    error ZeroAmount();
    error AlreadyExecuted();
    error NotEnoughSignatures(uint256 got, uint256 want);
    error InvalidSignerOrder();
    error UnknownAsset(bytes32 debridgeId);
    error BadReceiver();
    /// @dev the gate received a different amount than `amount` on transferIn — a
    ///      fee-on-transfer / rebasing token, which this gate does not support
    ///      (the signed `amount` would exceed what was actually locked, letting a
    ///      claim on the destination release more than was received).
    error UnsupportedTokenBehavior(uint256 expected, uint256 received);
    /// @dev more signatures supplied than there are validators — junk padding.
    error TooManySignatures(uint256 supplied, uint256 validatorCount);
    error ZeroValidator();
    error ZeroAddress();
    /// @dev threshold must always satisfy 0 < threshold <= validatorCount
    error InvalidThreshold(uint256 threshold, uint256 validatorCount);
    error EnforcedPause();
    error NotAuthorizedToPause();
    /// @dev refund asked for a submissionId this gate never emitted (or one
    ///      already refunded — `sentBy` is cleared on payout)
    error NotSent(bytes32 submissionId);
    error AlreadyRefunded(bytes32 submissionId);
    /// @dev the `token` passed to `refund` is not the one `debridgeId` commits to
    error TokenMismatch(bytes32 debridgeId, address token);

    modifier onlyOwner() {
        if (msg.sender != owner) revert NotOwner();
        _;
    }

    modifier whenNotPaused() {
        if (paused) revert EnforcedPause();
        _;
    }

    constructor(address[] memory validators, uint256 threshold_) {
        owner = msg.sender;
        emit OwnershipTransferred(address(0), msg.sender);

        for (uint256 i = 0; i < validators.length; i++) {
            address v = validators[i];
            if (v == address(0)) revert ZeroValidator();
            if (!isValidator[v]) {
                isValidator[v] = true;
                validatorCount++;
                emit ValidatorSet(v, true);
            }
        }

        // A zero (or unreachable) threshold is fatal: threshold == 0 would let
        // claim() pass with NO signatures; threshold > validatorCount freezes funds.
        if (threshold_ == 0 || threshold_ > validatorCount) {
            revert InvalidThreshold(threshold_, validatorCount);
        }
        threshold = threshold_;
        emit ThresholdSet(threshold_);
    }

    // ---------------------------------------------------------------------
    // Governance
    // ---------------------------------------------------------------------

    /// @notice Begin a two-step ownership handover (the new owner must accept).
    function transferOwnership(address newOwner) external onlyOwner {
        if (newOwner == address(0)) revert ZeroAddress();
        pendingOwner = newOwner;
        emit OwnershipTransferStarted(owner, newOwner);
    }

    /// @notice Complete an ownership handover. Two-step so a typo'd address can't
    ///         brick governance.
    function acceptOwnership() external {
        if (msg.sender != pendingOwner) revert NotOwner();
        emit OwnershipTransferred(owner, pendingOwner);
        owner = pendingOwner;
        pendingOwner = address(0);
    }

    function setValidator(address v, bool active) external onlyOwner {
        if (v == address(0)) revert ZeroValidator();
        if (active && !isValidator[v]) {
            isValidator[v] = true;
            validatorCount++;
        } else if (!active && isValidator[v]) {
            isValidator[v] = false;
            validatorCount--;
            // never let the active set fall below the threshold (liveness)
            if (validatorCount < threshold) revert InvalidThreshold(threshold, validatorCount);
        } else {
            return; // no-op: no state change, no event
        }
        emit ValidatorSet(v, active);
    }

    function setThreshold(uint256 t) external onlyOwner {
        if (t == 0 || t > validatorCount) revert InvalidThreshold(t, validatorCount);
        threshold = t;
        emit ThresholdSet(t);
    }

    /// @notice Register the local ERC-20 that backs `debridgeId` on this chain.
    function setLocalToken(bytes32 debridgeId, address localToken) external onlyOwner {
        tokenOf[debridgeId] = localToken;
        emit LocalTokenSet(debridgeId, localToken);
    }

    /// @notice Appoint (or clear) the guardian who can trip the circuit breaker.
    /// @dev    The guardian is a low-trust "stop button": it can pause but never
    ///         un-pause or move funds, so a compromised guardian can only cause a
    ///         (recoverable) liveness halt, not theft. Pass address(0) to revoke.
    function setGuardian(address newGuardian) external onlyOwner {
        guardian = newGuardian;
        emit GuardianSet(newGuardian);
    }

    /// @notice Halt `send`/`claim` in an incident. Callable by owner or guardian.
    function pause() external {
        if (msg.sender != owner && msg.sender != guardian) revert NotAuthorizedToPause();
        if (!paused) {
            paused = true;
            emit Paused(msg.sender);
        }
    }

    /// @notice Resume `send`/`claim`. Owner only — guardians can stop but not start.
    function unpause() external onlyOwner {
        if (paused) {
            paused = false;
            emit Unpaused(msg.sender);
        }
    }

    // ---------------------------------------------------------------------
    // Source side: lock + emit
    // ---------------------------------------------------------------------

    /// @notice Lock `amount` of `token` and emit a `Sent` event for validators.
    /// @param token      the ERC-20 to lock on this (source) chain
    /// @param amount     amount to bridge
    /// @param chainIdTo  destination chain id
    /// @param receiver   destination recipient. Its width is fixed by the target VM:
    ///                   20 bytes for an EVM address, or 32 bytes for a non-EVM
    ///                   account key (e.g. a Solana pubkey / SPL associated token
    ///                   account). Any other length is rejected so funds can't lock
    ///                   here against a receiver the target gate can't decode.
    /// @param autoParams empty bytes for none, or abi.encode(AutoParamsTo) for an
    ///                   execution payload
    function send(
        address token,
        uint256 amount,
        uint256 chainIdTo,
        bytes calldata receiver,
        bytes calldata autoParams
    ) external whenNotPaused returns (bytes32 submissionId) {
        if (amount == 0) revert ZeroAmount();
        // The receiver is only ever hashed and emitted here (never dereferenced on
        // this chain), but we still pin its width to the destination address size:
        // 20 = EVM address, 32 = Solana/non-EVM account key. A wrong length means a
        // malformed recipient, so reject rather than lock funds against garbage.
        if (receiver.length != 20 && receiver.length != 32) revert BadReceiver();

        uint256 nonce = nonceTo[chainIdTo];
        bytes32 debridgeId = BridgeHash.getDebridgeId(block.chainid, token);
        bytes memory nativeSender = abi.encodePacked(msg.sender);

        submissionId = _idFor(
            debridgeId, amount, block.chainid, chainIdTo, nonce, receiver, autoParams, nativeSender
        );

        // Effects BEFORE the external transfer (checks-effects-interactions):
        // reserve the nonce and emit before calling into `token`. Otherwise a
        // token with a transfer hook could reenter send(), read the same nonce,
        // and emit a colliding `Sent` — desyncing the off-chain nonce sequence.
        nonceTo[chainIdTo] = nonce + 1;
        // Bind the refund recipient at lock time. The monotonic per-corridor
        // nonce makes submissionId unique, so this never overwrites a live entry.
        sentBy[submissionId] = msg.sender;
        emit Sent(
            submissionId,
            debridgeId,
            amount,
            block.chainid,
            chainIdTo,
            receiver,
            nonce,
            autoParams,
            nativeSender,
            token
        );

        // Exact-transfer policy: credit only tokens whose balance delta equals the
        // signed `amount`. A fee-on-transfer / rebasing token would deliver less
        // than `amount` while the emitted event (and thus the destination claim)
        // promises the full `amount` — draining shared liquidity by the shortfall.
        // Reject rather than silently over-credit. (A reentrant transfer hook can
        // only make `received` differ, which also reverts, rolling back the nonce
        // and the emitted Sent.)
        uint256 balBefore = IERC20(token).balanceOf(address(this));
        IERC20(token).safeTransferFrom(msg.sender, address(this), amount);
        uint256 received = IERC20(token).balanceOf(address(this)) - balBefore;
        if (received != amount) revert UnsupportedTokenBehavior(amount, received);
    }

    // ---------------------------------------------------------------------
    // Target side: verify + execute (replay-safe)
    // ---------------------------------------------------------------------

    /// @notice Verify a threshold of validator signatures and release funds once.
    /// @dev    `signatures` MUST be sorted by recovered signer address, strictly
    ///         ascending. This both de-duplicates signers and bounds gas.
    /// @param nativeSender the packed source-chain sender; required to recompute
    ///                     the id when `autoParams` is non-empty (else ignored)
    function claim(
        bytes32 debridgeId,
        uint256 amount,
        uint256 chainIdFrom,
        uint256 nonce,
        bytes calldata receiver,
        bytes calldata autoParams,
        bytes calldata nativeSender,
        bytes[] calldata signatures
    ) external whenNotPaused returns (bytes32 submissionId) {
        submissionId = _idFor(
            debridgeId, amount, chainIdFrom, block.chainid, nonce, receiver, autoParams, nativeSender
        );

        if (executed[submissionId]) revert AlreadyExecuted();

        _verifySignatures(submissionId, signatures);

        // effects before interactions
        executed[submissionId] = true;

        address localToken = tokenOf[debridgeId];
        if (localToken == address(0)) revert UnknownAsset(debridgeId);
        address to = _toAddress(receiver);

        IERC20(localToken).safeTransfer(to, amount);

        emit Claimed(submissionId, debridgeId, to, amount);
    }

    // ---------------------------------------------------------------------
    // Refund path: cancel on the destination, then refund on the source
    // ---------------------------------------------------------------------
    //
    // A transfer can strand: the destination gate may lack liquidity for the
    // asset, the corridor may be de-listed after the funds were locked, or the
    // target chain may be down long enough that nobody ever claims. The locked
    // funds must be returnable — but a refund that merely waits out a timeout is
    // a DOUBLE-SPEND: the transfer's validator signatures still exist, so a
    // keeper can `claim()` on the destination in the same window the source pays
    // the refund, and the same tokens are released twice.
    //
    // So the two legs are ordered, and the ordering is enforced on-chain rather
    // than by any timing assumption:
    //
    //   1. `cancel()` on the DESTINATION burns `executed[submissionId]`. From
    //      that moment `claim()` reverts with AlreadyExecuted — the destination
    //      can never pay out, permanently and verifiably.
    //   2. Validators observe the resulting `Cancelled` event (an ordinary
    //      on-chain fact, attested exactly like a `Sent` is) and only then sign
    //      the refund digest.
    //   3. `refund()` on the SOURCE returns the funds.
    //
    // If a keeper wins the race and claims first, step 1 simply reverts and no
    // refund is ever authorised. There is no interleaving that pays out twice.

    /// @notice DESTINATION side. Burn a transfer so it can never be claimed here,
    ///         unlocking a source-chain refund. Moves no funds.
    /// @dev    Requires a threshold of validator signatures over
    ///         `BridgeHash.getCancelId(submissionId)` — a distinct signing domain,
    ///         so the validators' original transfer signatures (which authorise
    ///         *paying* this submissionId) can never be replayed to burn it.
    function cancel(
        bytes32 debridgeId,
        uint256 amount,
        uint256 chainIdFrom,
        uint256 nonce,
        bytes calldata receiver,
        bytes calldata autoParams,
        bytes calldata nativeSender,
        bytes[] calldata signatures
    ) external whenNotPaused returns (bytes32 submissionId) {
        submissionId = _idFor(
            debridgeId, amount, chainIdFrom, block.chainid, nonce, receiver, autoParams, nativeSender
        );

        // Already claimed (or already cancelled) — either way it is spent here,
        // and re-cancelling must not re-authorise a second refund.
        if (executed[submissionId]) revert AlreadyExecuted();

        _verifySignatures(BridgeHash.getCancelId(submissionId), signatures);

        executed[submissionId] = true;
        cancelled[submissionId] = true;

        emit Cancelled(submissionId, debridgeId, chainIdFrom, nonce);
    }

    /// @notice SOURCE side. Return locked funds to the original sender after the
    ///         destination has been cancelled.
    /// @dev    Three independent guards stand between a caller and the vault:
    ///           * `sentBy[submissionId]` must be set — proof THIS gate locked
    ///             these funds, and the authoritative recipient (the calldata's
    ///             `nativeSender` is not hash-bound for a plain transfer, so it is
    ///             never trusted for the payout address);
    ///           * a validator threshold over `BridgeHash.getRefundId(...)`, whose
    ///             quorum only forms after `Cancelled` is observed on the target;
    ///           * `token` must be exactly the asset `debridgeId` commits to.
    /// @param token the ERC-20 originally locked. `debridgeId` is a one-way hash
    ///        of it, so it is supplied by the caller and checked here rather than
    ///        stored — untrusted input, exactly verified.
    function refund(
        address token,
        bytes32 debridgeId,
        uint256 amount,
        uint256 chainIdTo,
        uint256 nonce,
        bytes calldata receiver,
        bytes calldata autoParams,
        bytes calldata nativeSender,
        bytes[] calldata signatures
    ) external whenNotPaused returns (bytes32 submissionId) {
        submissionId = _idFor(
            debridgeId, amount, block.chainid, chainIdTo, nonce, receiver, autoParams, nativeSender
        );

        if (refunded[submissionId]) revert AlreadyRefunded(submissionId);

        // Origin proof AND payout address, both from storage this gate wrote at
        // lock time. Zero means "we never sent this" (or it is already refunded).
        address sender = sentBy[submissionId];
        if (sender == address(0)) revert NotSent(submissionId);

        // debridgeId = keccak(thisChain, token): an exact binding, so a caller
        // cannot name a different (more valuable) asset held by this gate.
        if (BridgeHash.getDebridgeId(block.chainid, token) != debridgeId) {
            revert TokenMismatch(debridgeId, token);
        }

        _verifySignatures(BridgeHash.getRefundId(submissionId), signatures);

        // effects before interactions
        refunded[submissionId] = true;
        delete sentBy[submissionId];

        IERC20(token).safeTransfer(sender, amount);

        emit Refunded(submissionId, debridgeId, sender, amount);
    }

    /// @notice Recompute a submissionId without executing (hash-equivalence tests).
    function computeSubmissionId(
        bytes32 debridgeId,
        uint256 amount,
        uint256 chainIdFrom,
        uint256 chainIdTo,
        uint256 nonce,
        bytes calldata receiver,
        bytes calldata autoParams,
        bytes calldata nativeSender
    ) external pure returns (bytes32) {
        return _idFor(
            debridgeId, amount, chainIdFrom, chainIdTo, nonce, receiver, autoParams, nativeSender
        );
    }

    // ---------------------------------------------------------------------
    // Internal
    // ---------------------------------------------------------------------

    function _idFor(
        bytes32 debridgeId,
        uint256 amount,
        uint256 chainIdFrom,
        uint256 chainIdTo,
        uint256 nonce,
        bytes memory receiver,
        bytes memory autoParams,
        bytes memory nativeSender
    ) internal pure returns (bytes32) {
        if (autoParams.length == 0) {
            return BridgeHash.getSubmissionId(
                debridgeId, amount, chainIdFrom, chainIdTo, nonce, receiver
            );
        }
        AutoParamsTo memory ap = abi.decode(autoParams, (AutoParamsTo));
        return BridgeHash.getSubmissionIdWithAuto(
            debridgeId,
            amount,
            chainIdFrom,
            chainIdTo,
            nonce,
            receiver,
            BridgeHash.AutoParams({
                executionFee: ap.executionFee,
                flags: ap.flags,
                fallbackAddress: ap.fallbackAddress,
                data: ap.data,
                nativeSender: nativeSender
            })
        );
    }

    /// @dev Verify a validator threshold over the EIP-191 `eth_sign` digest of a
    ///      raw 32-byte message.
    /// @param message one of three domain-separated values — a `submissionId`
    ///        (authorises paying a transfer out), a `cancelId` (authorises
    ///        burning it on the destination), or a `refundId` (authorises
    ///        returning it on the source). `BridgeHash` derives the latter two
    ///        under distinct prefixes, so a quorum for one is never a quorum for
    ///        another.
    function _verifySignatures(bytes32 message, bytes[] calldata signatures)
        internal
        view
    {
        bytes32 digest = MessageHashUtils.toEthSignedMessageHash(message);

        // A useful quorum needs at most `validatorCount` distinct signers (the
        // ascending-order rule already forbids duplicates, and non-validators are
        // ignored below). Cap the array there so a caller can't pad it with junk
        // to inflate ECDSA-recovery gas / RPC estimation load.
        if (signatures.length > validatorCount) {
            revert TooManySignatures(signatures.length, validatorCount);
        }

        address last = address(0);
        uint256 count = 0;
        for (uint256 i = 0; i < signatures.length; i++) {
            address signer = ECDSA.recover(digest, signatures[i]);
            // strictly ascending => distinct signers, no duplicates
            if (signer <= last) revert InvalidSignerOrder();
            if (isValidator[signer]) {
                count++;
            }
            last = signer;
        }
        if (count < threshold) revert NotEnoughSignatures(count, threshold);
    }

    /// @dev Read the first 20 bytes of `receiver` as an EVM address.
    function _toAddress(bytes calldata receiver) internal pure returns (address addr) {
        if (receiver.length < 20) revert BadReceiver();
        addr = address(bytes20(receiver[0:20]));
    }
}
