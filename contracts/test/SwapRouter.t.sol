// SPDX-License-Identifier: MIT
pragma solidity 0.8.24;

import {Test} from "forge-std/Test.sol";
import {Gate} from "../src/Gate.sol";
import {SwapPool} from "../src/SwapPool.sol";
import {SwapRouter} from "../src/SwapRouter.sol";
import {BridgeHash} from "../src/BridgeHash.sol";
import {MessageHashUtils} from "@openzeppelin/contracts/utils/cryptography/MessageHashUtils.sol";
import {ERC20} from "@openzeppelin/contracts/token/ERC20/ERC20.sol";

/// @dev Mintable ERC-20 with configurable decimals (same helper the deploy uses).
contract MockToken is ERC20 {
    uint8 private immutable _dec;

    constructor(string memory n, string memory s, uint8 d) ERC20(n, s) {
        _dec = d;
    }

    function decimals() public view override returns (uint8) {
        return _dec;
    }

    function mint(address to, uint256 amt) external {
        _mint(to, amt);
    }
}

/// @notice End-to-end cross-chain swap over the SwapRouter, simulating TWO chains
///         in one EVM via `vm.chainId`. A user on chain A swaps WETH into TT on
///         chain B: WETH --poolA--> stable --Gate--> stable --poolB--> TT.
///
///         Neither Gate nor SwapPool is modified; the destination leg is trustless
///         (proven by Gate.executed[submissionId]) with a stable-refund fallback.
contract SwapRouterTest is Test {
    // two "chains"
    uint256 constant CHAIN_A = 1337;
    uint256 constant CHAIN_B = 8453;
    uint16 constant DEVIATION_BPS = 1000;

    // one validator, threshold 1 (signature machinery proven in Claim.t.sol)
    uint256 v1pk = 0xA11CE;
    address v1;

    // chain A
    Gate gateA;
    SwapPool poolA;
    SwapRouter routerA;
    MockToken usdA; // 6-dec stable
    MockToken weth; // 18-dec, priced 3180

    // chain B
    Gate gateB;
    SwapPool poolB;
    SwapRouter routerB;
    MockToken usdB; // 6-dec stable
    MockToken tt; // 18-dec, priced 2

    address user = address(0xB0B);
    address finalReceiver = address(0xBEEF);

    uint256 constant WETH_PRICE = 3180e18;
    uint256 constant TT_PRICE = 2e18;

    function setUp() public {
        v1 = vm.addr(v1pk);
        address[] memory vals = new address[](1);
        vals[0] = v1;

        // --- chain A ---
        vm.chainId(CHAIN_A);
        gateA = new Gate(vals, 1);
        usdA = new MockToken("USD A", "USDa", 6);
        weth = new MockToken("Wrapped Ether", "WETH", 18);
        poolA = new SwapPool(address(usdA), DEVIATION_BPS);
        poolA.listToken(address(weth), WETH_PRICE);
        _seed(poolA, usdA, 10_000_000e6);
        _seed(poolA, weth, 100e18);
        routerA = new SwapRouter(gateA, poolA);

        // --- chain B ---
        vm.chainId(CHAIN_B);
        gateB = new Gate(vals, 1);
        usdB = new MockToken("USD B", "USDb", 6);
        tt = new MockToken("Test Token", "TT", 18);
        poolB = new SwapPool(address(usdB), DEVIATION_BPS);
        poolB.listToken(address(tt), TT_PRICE);
        _seed(poolB, usdB, 10_000_000e6);
        _seed(poolB, tt, 1_000_000e18);
        routerB = new SwapRouter(gateB, poolB);

        // wire the corridor A <-> B
        routerA.setRemoteRouter(CHAIN_B, abi.encodePacked(address(routerB)));
        routerB.setRemoteRouter(CHAIN_A, abi.encodePacked(address(routerA)));

        // The stable bridges A->B as (native chain A, usdA) -> local usdB. Register
        // the mapping on B and pre-fund gateB with target-side stable liquidity.
        bytes32 stableDid = BridgeHash.getDebridgeId(CHAIN_A, address(usdA));
        gateB.setLocalToken(stableDid, address(usdB));
        usdB.mint(address(gateB), 10_000_000e6);
    }

    function _seed(SwapPool pool, MockToken token, uint256 amt) internal {
        token.mint(address(this), amt);
        token.approve(address(pool), amt);
        pool.seedLiquidity(address(token), amt);
    }

    function _sign(uint256 pk, bytes32 id) internal pure returns (bytes[] memory sigs) {
        bytes32 digest = MessageHashUtils.toEthSignedMessageHash(id);
        (uint8 vv, bytes32 r, bytes32 s) = vm.sign(pk, digest);
        sigs = new bytes[](1);
        sigs[0] = abi.encodePacked(r, s, vv);
    }

    // The bridged transfer's fields, as they appear in the source `Sent` event.
    struct Leg {
        bytes32 debridgeId;
        uint256 amount; // stable bridged (= poolA WETH->stable output)
        uint256 nonce;
        bytes receiver; // routerB
        bytes autoParams;
        bytes nativeSender; // routerA
        bytes32 id;
    }

    /// @dev Run the source leg on chain A and reconstruct the resulting transfer.
    function _sourceLeg(uint256 amountIn, address finalToken, uint256 finalMinOut)
        internal
        returns (Leg memory leg)
    {
        vm.chainId(CHAIN_A);
        weth.mint(user, amountIn);
        vm.startPrank(user);
        weth.approve(address(routerA), amountIn);
        leg.id = routerA.swapAndBridge(
            address(weth), amountIn, 0, CHAIN_B, finalToken, finalReceiver, finalMinOut
        );
        vm.stopPrank();

        // Reconstruct the transfer fields deterministically (asserts our encoding).
        leg.debridgeId = BridgeHash.getDebridgeId(CHAIN_A, address(usdA));
        leg.amount = poolA.quote(address(weth), address(usdA), amountIn);
        leg.nonce = 0; // first send A->B
        leg.receiver = abi.encodePacked(address(routerB));
        leg.nativeSender = abi.encodePacked(address(routerA));
        Gate.AutoParamsTo memory ap = Gate.AutoParamsTo({
            executionFee: 0,
            flags: 0,
            fallbackAddress: abi.encodePacked(finalReceiver),
            data: abi.encode(finalToken, finalReceiver, finalMinOut)
        });
        leg.autoParams = abi.encode(ap);

        // the id the router returned must equal the canonical id we rebuild
        bytes32 rebuilt = gateA.computeSubmissionId(
            leg.debridgeId, leg.amount, CHAIN_A, CHAIN_B, leg.nonce, leg.receiver, leg.autoParams, leg.nativeSender
        );
        assertEq(leg.id, rebuilt, "source id mismatch");
    }

    // ------------------------------------------------------------------
    // Happy path: WETH@A -> TT@B, one atomic claimAndFinalize on B
    // ------------------------------------------------------------------
    function test_CrossChain_SwapAndBridge_ClaimAndFinalize() public {
        uint256 amountIn = 1e18; // 1 WETH
        Leg memory leg = _sourceLeg(amountIn, address(tt), 0);

        // 3180 USD of stable bridged (WETH price 3180, stable 6-dec)
        assertEq(leg.amount, 3180e6, "bridged stable wrong");

        // expected TT out on B: 3180 USD / 2 = 1590 TT
        uint256 expectedTt = poolB.quote(address(usdB), address(tt), leg.amount);
        assertEq(expectedTt, 1590e18, "dest quote wrong");

        vm.chainId(CHAIN_B);
        bytes[] memory sigs = _sign(v1pk, leg.id);
        bytes32 got = routerB.claimAndFinalize(
            leg.debridgeId, leg.amount, CHAIN_A, leg.nonce, leg.receiver, leg.autoParams, leg.nativeSender, sigs
        );

        assertEq(got, leg.id, "finalized id mismatch");
        assertEq(tt.balanceOf(finalReceiver), expectedTt, "final receiver not paid in TT");
        assertTrue(gateB.executed(leg.id), "claim not recorded");
        assertTrue(routerB.finalized(leg.id), "finalize not recorded");
        // router holds no residual stable
        assertEq(usdB.balanceOf(address(routerB)), 0, "stable stranded at router");
    }

    // ------------------------------------------------------------------
    // A cancelled transfer is NOT a delivered one
    // ------------------------------------------------------------------
    function test_Finalize_AfterCancel_Reverts() public {
        // `Gate.cancel` burns a stranded transfer by setting `executed` — without
        // ever releasing the stable. `finalize` used to read `executed` as proof
        // of delivery, so it would have paid `finalReceiver` out of whatever
        // liquidity happened to be resting at the router (another user's in-flight
        // transfer), while the source chain separately refunded the sender.
        Leg memory leg = _sourceLeg(1e18, address(tt), 0);

        vm.chainId(CHAIN_B);
        bytes32 cancelId = BridgeHash.getCancelId(leg.id);
        gateB.cancel(
            leg.debridgeId, leg.amount, CHAIN_A, leg.nonce, leg.receiver, leg.autoParams,
            leg.nativeSender, _sign(v1pk, cancelId)
        );
        assertTrue(gateB.executed(leg.id), "cancel did not burn the transfer");
        assertTrue(gateB.cancelled(leg.id), "cancelled flag not set");

        // strand some stable at the router, so a buggy finalize WOULD have paid out
        usdB.mint(address(routerB), leg.amount);

        vm.expectRevert(abi.encodeWithSelector(SwapRouter.NotDelivered.selector, leg.id));
        routerB.finalize(
            leg.debridgeId, leg.amount, CHAIN_A, leg.nonce, leg.receiver, leg.autoParams, leg.nativeSender
        );

        assertEq(tt.balanceOf(finalReceiver), 0, "receiver paid for a cancelled transfer");
        assertFalse(routerB.finalized(leg.id), "cancelled transfer marked finalized");
    }

    // ------------------------------------------------------------------
    // Two-step: keeper claims via the Gate, then anyone calls finalize
    // ------------------------------------------------------------------
    function test_CrossChain_Finalize_AfterPlainClaim() public {
        Leg memory leg = _sourceLeg(1e18, address(tt), 0);

        vm.chainId(CHAIN_B);
        bytes[] memory sigs = _sign(v1pk, leg.id);
        // a keeper claims into the router (stable released to routerB)
        gateB.claim(
            leg.debridgeId, leg.amount, CHAIN_A, leg.nonce, leg.receiver, leg.autoParams, leg.nativeSender, sigs
        );
        assertEq(usdB.balanceOf(address(routerB)), leg.amount, "stable not delivered to router");

        // a permissionless finalize completes the swap
        vm.prank(address(0xDEAD));
        routerB.finalize(
            leg.debridgeId, leg.amount, CHAIN_A, leg.nonce, leg.receiver, leg.autoParams, leg.nativeSender
        );
        assertEq(tt.balanceOf(finalReceiver), 1590e18, "final swap did not run");
        assertEq(usdB.balanceOf(address(routerB)), 0, "stable not fully consumed");
    }

    // ------------------------------------------------------------------
    // Finalize before delivery must revert (no free swaps)
    // ------------------------------------------------------------------
    function test_Finalize_NotDelivered_Reverts() public {
        Leg memory leg = _sourceLeg(1e18, address(tt), 0);

        vm.chainId(CHAIN_B);
        vm.expectRevert(abi.encodeWithSelector(SwapRouter.NotDelivered.selector, leg.id));
        routerB.finalize(
            leg.debridgeId, leg.amount, CHAIN_A, leg.nonce, leg.receiver, leg.autoParams, leg.nativeSender
        );
    }

    // ------------------------------------------------------------------
    // Idempotency: a second finalize on the same transfer reverts
    // ------------------------------------------------------------------
    function test_Finalize_Idempotent() public {
        Leg memory leg = _sourceLeg(1e18, address(tt), 0);

        vm.chainId(CHAIN_B);
        bytes[] memory sigs = _sign(v1pk, leg.id);
        routerB.claimAndFinalize(
            leg.debridgeId, leg.amount, CHAIN_A, leg.nonce, leg.receiver, leg.autoParams, leg.nativeSender, sigs
        );

        vm.expectRevert(abi.encodeWithSelector(SwapRouter.AlreadyFinalized.selector, leg.id));
        routerB.finalize(
            leg.debridgeId, leg.amount, CHAIN_A, leg.nonce, leg.receiver, leg.autoParams, leg.nativeSender
        );
    }

    // ------------------------------------------------------------------
    // Fallback: if the destination swap can't complete, deliver the stable
    // ------------------------------------------------------------------
    function test_Finalize_Fallback_DeliversStable() public {
        // Ask for more TT than the pool's reserve can pay -> pool reverts
        // ExceedsLock inside finalize -> fallback refunds the stable instead.
        // Bridge a big amount so the TT output would exceed a tiny TT reserve.
        // First, shrink poolB's TT reserve to force the lock.
        vm.chainId(CHAIN_B);
        poolB.withdrawLiquidity(address(tt), 1_000_000e18 - 1e18, address(this)); // leave 1 TT

        Leg memory leg = _sourceLeg(1e18, address(tt), 0); // wants 1590 TT, only 1 left

        vm.chainId(CHAIN_B);
        bytes[] memory sigs = _sign(v1pk, leg.id);
        routerB.claimAndFinalize(
            leg.debridgeId, leg.amount, CHAIN_A, leg.nonce, leg.receiver, leg.autoParams, leg.nativeSender, sigs
        );

        // no TT paid; the stable was refunded to the final receiver instead
        assertEq(tt.balanceOf(finalReceiver), 0, "should not have paid TT");
        assertEq(usdB.balanceOf(finalReceiver), leg.amount, "stable fallback not delivered");
        assertEq(usdB.balanceOf(address(routerB)), 0, "stable stranded after fallback");
        assertTrue(routerB.finalized(leg.id), "finalize should be recorded even on fallback");
    }

    // ------------------------------------------------------------------
    // finalToken == stable degenerate intent: deliver stable, no swap
    // ------------------------------------------------------------------
    function test_Finalize_StableIntent_DeliversStable() public {
        Leg memory leg = _sourceLeg(1e18, address(usdB), 0); // wants the stable itself

        vm.chainId(CHAIN_B);
        bytes[] memory sigs = _sign(v1pk, leg.id);
        routerB.claimAndFinalize(
            leg.debridgeId, leg.amount, CHAIN_A, leg.nonce, leg.receiver, leg.autoParams, leg.nativeSender, sigs
        );
        assertEq(usdB.balanceOf(finalReceiver), leg.amount, "stable intent not delivered");
    }

    // ------------------------------------------------------------------
    // Access / config
    // ------------------------------------------------------------------
    function test_SwapAndBridge_UnconfiguredRoute_Reverts() public {
        vm.chainId(CHAIN_A);
        uint256 unknownChain = 999;
        weth.mint(user, 1e18);
        vm.startPrank(user);
        weth.approve(address(routerA), 1e18);
        vm.expectRevert(abi.encodeWithSelector(SwapRouter.RouteNotConfigured.selector, unknownChain));
        routerA.swapAndBridge(address(weth), 1e18, 0, unknownChain, address(tt), finalReceiver, 0);
        vm.stopPrank();
    }

    function test_SetRemoteRouter_OnlyOwner() public {
        vm.prank(address(0xBAD));
        vm.expectRevert(SwapRouter.NotOwner.selector);
        routerA.setRemoteRouter(CHAIN_B, abi.encodePacked(address(0x1234)));
    }
}
