// SPDX-License-Identifier: MIT
pragma solidity 0.8.24;

import {Test} from "forge-std/Test.sol";
import {SwapPool} from "../src/SwapPool.sol";
import {ERC20} from "@openzeppelin/contracts/token/ERC20/ERC20.sol";
import {ReentrancyGuard} from "@openzeppelin/contracts/utils/ReentrancyGuard.sol";

/// @dev ERC-20 with configurable decimals (to exercise the swap's decimal
///      normalisation — e.g. a 6-dec stablecoin against 18-dec tokens).
contract MockERC20 is ERC20 {
    uint8 private immutable _dec;

    constructor(string memory n, string memory s, uint8 d) ERC20(n, s) {
        _dec = d;
    }

    function decimals() public view override returns (uint8) {
        return _dec;
    }

    function mint(address to, uint256 amount) external {
        _mint(to, amount);
    }
}

/// @dev A token whose `transfer` reenters `SwapPool.swap` once, to prove the
///      nonReentrant guard blocks reentrancy on the outgoing leg.
contract ReenterToken is ERC20 {
    SwapPool public pool;
    address public tokenIn;
    bool public entered;

    constructor() ERC20("Re", "RE") {}

    function decimals() public pure override returns (uint8) {
        return 18;
    }

    function mint(address to, uint256 amount) external {
        _mint(to, amount);
    }

    function arm(SwapPool p, address tokenIn_) external {
        pool = p;
        tokenIn = tokenIn_;
    }

    function transfer(address to, uint256 amount) public override returns (bool) {
        if (address(pool) != address(0) && !entered) {
            entered = true;
            // reenter — must hit the ReentrancyGuard and revert the whole swap
            pool.swap(tokenIn, address(this), 1, 0, address(this));
        }
        return super.transfer(to, amount);
    }
}

contract SwapTest is Test {
    SwapPool pool;

    MockERC20 usd; // stable, 6 decimals, price 1.0
    MockERC20 weth; // 18 decimals, price 3180
    MockERC20 tt; // 18 decimals, price 1.0

    address user = address(0x115E7);
    address oracle = address(0x0AC1E);
    address guardian = address(0x6A5D);
    address attacker = address(0xBAD);

    uint256 constant PRICE_ONE = 1e18;
    uint16 constant DEVIATION = 1000; // 10%

    uint256 constant WETH_PRICE = 3180e18;
    uint256 constant TT_PRICE = 1e18;

    function setUp() public {
        usd = new MockERC20("Mock USD", "mUSD", 6);
        weth = new MockERC20("Wrapped Ether", "WETH", 18);
        tt = new MockERC20("Test Token", "TT", 18);

        pool = new SwapPool(address(usd), DEVIATION);
        pool.setOracle(oracle);

        pool.listToken(address(weth), WETH_PRICE);
        pool.listToken(address(tt), TT_PRICE);

        // Seed reserves (the locks).
        _seed(usd, 1_000_000e6); // 1,000,000 mUSD
        _seed(weth, 100e18); // 100 WETH
        _seed(tt, 500_000e18); // 500,000 TT
    }

    function _seed(MockERC20 token, uint256 amount) internal {
        token.mint(address(this), amount);
        token.approve(address(pool), amount);
        pool.seedLiquidity(address(token), amount);
    }

    function _fund(MockERC20 token, address who, uint256 amount) internal {
        token.mint(who, amount);
        vm.prank(who);
        token.approve(address(pool), type(uint256).max);
    }

    // ----------------------------------------------------------------- quote

    function test_Quote_MixedDecimals() public view {
        // 1 WETH -> mUSD == 3180.000000
        assertEq(pool.quote(address(weth), address(usd), 1e18), 3180e6);
        // 3180 mUSD -> 1 WETH
        assertEq(pool.quote(address(usd), address(weth), 3180e6), 1e18);
        // 1 WETH -> TT == 3180 TT (both 18-dec)
        assertEq(pool.quote(address(weth), address(tt), 1e18), 3180e18);
    }

    // ----------------------------------------------------------------- swaps

    function test_Swap_TokenToStable() public {
        _fund(weth, user, 1e18);
        vm.prank(user);
        uint256 out = pool.swap(address(weth), address(usd), 1e18, 0, user);
        assertEq(out, 3180e6);
        assertEq(usd.balanceOf(user), 3180e6);
        (,, , uint256 wReserve) = _info(address(weth));
        assertEq(wReserve, 101e18); // 100 + 1
        (,, , uint256 uReserve) = _info(address(usd));
        assertEq(uReserve, 1_000_000e6 - 3180e6);
    }

    function test_Swap_StableToToken() public {
        _fund(usd, user, 3180e6);
        vm.prank(user);
        uint256 out = pool.swap(address(usd), address(weth), 3180e6, 0, user);
        assertEq(out, 1e18);
        assertEq(weth.balanceOf(user), 1e18);
    }

    function test_Swap_TokenToToken() public {
        _fund(weth, user, 1e18);
        vm.prank(user);
        uint256 out = pool.swap(address(weth), address(tt), 1e18, 0, user);
        assertEq(out, 3180e18);
        assertEq(tt.balanceOf(user), 3180e18);
    }

    // --------------------------------------------------------- the lock / cap

    function test_Swap_ExceedsLock_Reverts() public {
        // buying 100.01 WETH needs > 100 WETH reserve -> ExceedsLock
        uint256 amountIn = 318032e6; // 318,032 mUSD -> ~100.01 WETH
        _fund(usd, user, amountIn);
        uint256 out = pool.quote(address(usd), address(weth), amountIn);
        assertGt(out, 100e18);
        vm.prank(user);
        vm.expectRevert(abi.encodeWithSelector(SwapPool.ExceedsLock.selector, out, 100e18));
        pool.swap(address(usd), address(weth), amountIn, 0, user);
    }

    function test_Swap_AtLockBoundary_Succeeds() public {
        // buying exactly 100 WETH (== reserve) must succeed and zero the reserve
        uint256 amountIn = 318_000e6; // 318,000 mUSD -> exactly 100 WETH
        _fund(usd, user, amountIn);
        vm.prank(user);
        uint256 out = pool.swap(address(usd), address(weth), amountIn, 0, user);
        assertEq(out, 100e18);
        (,, , uint256 wReserve) = _info(address(weth));
        assertEq(wReserve, 0);

        // now the pool is dry for WETH — any further WETH buy reverts
        _fund(usd, attacker, 3180e6);
        vm.prank(attacker);
        vm.expectRevert(abi.encodeWithSelector(SwapPool.ExceedsLock.selector, 1e18, 0));
        pool.swap(address(usd), address(weth), 3180e6, 0, attacker);
    }

    // ------------------------------------------------------------- slippage

    function test_Swap_Slippage_Reverts() public {
        _fund(weth, user, 1e18);
        vm.prank(user);
        vm.expectRevert(abi.encodeWithSelector(SwapPool.Slippage.selector, 3180e6, 3181e6));
        pool.swap(address(weth), address(usd), 1e18, 3181e6, user);
    }

    // ------------------------------------------------- rounding favors pool

    function test_Swap_RoundTrip_NeverProfits() public {
        // TT (18-dec) -> mUSD (6-dec) loses sub-1e-6 dust; round-trip <= input.
        uint256 amountIn = 1e18 + 5e11; // 1.0000005 TT
        _fund(tt, user, amountIn);

        vm.startPrank(user);
        uint256 got = pool.swap(address(tt), address(usd), amountIn, 0, user);
        usd.approve(address(pool), got);
        uint256 back = pool.swap(address(usd), address(tt), got, 0, user);
        vm.stopPrank();

        assertLe(back, amountIn, "round-trip must not create value");
        assertLt(back, amountIn, "the 6-dec hop should drop dust");
        assertEq(back, 1e18);
    }

    // ------------------------------------------------------------- pricing

    function test_SetPrice_OracleWithinDeviation() public {
        vm.prank(oracle);
        pool.setPrice(address(weth), 3498e18); // +10%, exactly at the cap
        (, , uint256 price, ) = _info(address(weth));
        assertEq(price, 3498e18);
    }

    function test_SetPrice_RevertsDeviationTooHigh() public {
        vm.prank(oracle);
        vm.expectRevert(
            abi.encodeWithSelector(SwapPool.PriceDeviationTooHigh.selector, 3180e18, 3600e18, DEVIATION)
        );
        pool.setPrice(address(weth), 3600e18); // +13.2% > 10%
    }

    function test_SetPrice_OnlyOracle() public {
        vm.prank(attacker);
        vm.expectRevert(SwapPool.NotOracle.selector);
        pool.setPrice(address(weth), 3200e18);
    }

    function test_SetPrice_StableForbidden() public {
        vm.prank(oracle);
        vm.expectRevert(SwapPool.StableRepriceForbidden.selector);
        pool.setPrice(address(usd), 2e18);
    }

    // ----------------------------------------------------------------- fees

    function test_Fee_ReducesOutput() public {
        pool.setFee(100); // 1%
        // 1 WETH -> mUSD: 3180 * 0.99 = 3148.2
        assertEq(pool.quote(address(weth), address(usd), 1e18), 3148_200000);
        _fund(weth, user, 1e18);
        vm.prank(user);
        uint256 out = pool.swap(address(weth), address(usd), 1e18, 0, user);
        assertEq(out, 3148_200000);
    }

    function test_SetFee_TooHigh() public {
        vm.expectRevert(SwapPool.FeeTooHigh.selector);
        pool.setFee(1001);
    }

    function test_SetFee_OnlyOwner() public {
        vm.prank(attacker);
        vm.expectRevert(SwapPool.NotOwner.selector);
        pool.setFee(50);
    }

    // -------------------------------------------------------------- pausing

    function test_Pause_HaltsSwap() public {
        _fund(weth, user, 1e18);
        pool.pause();
        vm.prank(user);
        vm.expectRevert(SwapPool.EnforcedPause.selector);
        pool.swap(address(weth), address(usd), 1e18, 0, user);
    }

    function test_Guardian_CanPauseNotUnpause() public {
        pool.setGuardian(guardian);
        vm.prank(guardian);
        pool.pause();
        assertTrue(pool.paused());

        vm.prank(guardian);
        vm.expectRevert(SwapPool.NotOwner.selector);
        pool.unpause();

        pool.unpause();
        assertFalse(pool.paused());
    }

    function test_Pause_OnlyOwnerOrGuardian() public {
        vm.prank(attacker);
        vm.expectRevert(SwapPool.NotAuthorizedToPause.selector);
        pool.pause();
    }

    // ---------------------------------------------------------- reentrancy

    function test_Swap_ReentrancyBlocked() public {
        ReenterToken re = new ReenterToken();
        pool.listToken(address(re), 1e18);
        re.mint(address(this), 1000e18);
        re.approve(address(pool), type(uint256).max);
        pool.seedLiquidity(address(re), 1000e18);
        re.arm(pool, address(usd));

        _fund(usd, user, 100e6);
        vm.prank(user);
        // the outgoing transfer of `re` reenters swap() -> guard reverts the tx
        vm.expectRevert();
        pool.swap(address(usd), address(re), 100e6, 0, user);
    }

    // ------------------------------------------------------- access control

    function test_ListToken_OnlyOwner() public {
        MockERC20 x = new MockERC20("X", "X", 18);
        vm.prank(attacker);
        vm.expectRevert(SwapPool.NotOwner.selector);
        pool.listToken(address(x), 1e18);
    }

    function test_ListToken_AlreadyListed() public {
        vm.expectRevert(abi.encodeWithSelector(SwapPool.TokenAlreadyListed.selector, address(weth)));
        pool.listToken(address(weth), 1e18);
    }

    function test_ListToken_StableAlreadyListed() public {
        vm.expectRevert(abi.encodeWithSelector(SwapPool.TokenAlreadyListed.selector, address(usd)));
        pool.listToken(address(usd), 1e18);
    }

    function test_SeedAndWithdraw_OnlyOwner() public {
        vm.startPrank(attacker);
        vm.expectRevert(SwapPool.NotOwner.selector);
        pool.seedLiquidity(address(weth), 1e18);
        vm.expectRevert(SwapPool.NotOwner.selector);
        pool.withdrawLiquidity(address(weth), 1e18, attacker);
        vm.stopPrank();
    }

    function test_Withdraw_ExceedsReserve() public {
        vm.expectRevert(abi.encodeWithSelector(SwapPool.ExceedsLock.selector, 101e18, 100e18));
        pool.withdrawLiquidity(address(weth), 101e18, address(this));
    }

    function test_Delist_RequiresZeroReserve() public {
        vm.expectRevert(SwapPool.ReserveNonZero.selector);
        pool.delistToken(address(weth));

        pool.withdrawLiquidity(address(weth), 100e18, address(this));
        pool.delistToken(address(weth));
        (bool listed,,,) = _info(address(weth));
        assertFalse(listed);
    }

    function test_MaxSwapOut() public view {
        (uint256 reserve, uint256 usdValue) = pool.maxSwapOut(address(weth));
        assertEq(reserve, 100e18);
        assertEq(usdValue, 318_000e18); // 100 WETH * 3180 = 318,000 USD (1e18-scaled)
    }

    // ----------------------------------------------------------- ownership

    function test_TransferOwnership_TwoStep() public {
        address newOwner = address(0xBEEF);
        pool.transferOwnership(newOwner);
        assertEq(pool.owner(), address(this));
        vm.prank(newOwner);
        pool.acceptOwnership();
        assertEq(pool.owner(), newOwner);
    }

    // helper: unpack the public mapping getter
    function _info(address token)
        internal
        view
        returns (bool listed, uint8 dec, uint256 price, uint256 reserve)
    {
        (listed, dec, price, reserve) = pool.tokens(token);
    }
}
