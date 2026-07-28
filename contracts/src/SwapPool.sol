// SPDX-License-Identifier: MIT
pragma solidity 0.8.24;

import {IERC20} from "@openzeppelin/contracts/token/ERC20/IERC20.sol";
import {IERC20Metadata} from "@openzeppelin/contracts/token/ERC20/extensions/IERC20Metadata.sol";
import {SafeERC20} from "@openzeppelin/contracts/token/ERC20/utils/SafeERC20.sol";
import {ReentrancyGuard} from "@openzeppelin/contracts/utils/ReentrancyGuard.sol";
import {Math} from "@openzeppelin/contracts/utils/math/Math.sol";

/// @title SwapPool
/// @notice A same-chain, pegged-price swap pool for bridge tokens. Every listed
///         token has an admin/oracle-set USD price; ONE stablecoin is the core
///         unit of account (price fixed at 1.0, immutable). A swap converts
///         `tokenIn -> tokenOut` at the pegged rate, normalising for token
///         decimals, and is HARD-CAPPED by the output token's locked reserve —
///         so a swap can never drain a pool ("max swap up to token lock").
/// @dev    Deliberately mirrors Gate.sol's security idioms (SafeERC20, CEI,
///         two-step ownership, guardian + paused circuit breaker, custom errors,
///         rich events). Liquidity is PROTOCOL-OWNED: the owner seeds/withdraws
///         reserves; there are no LP shares in v1. Reserves are tracked
///         INTERNALLY (not via balanceOf) so a raw token donation cannot corrupt
///         pricing or the cap, and fee-on-transfer tokens are caught by delta.
///
///         This is the same-chain primitive. A future cross-chain SwapRouter can
///         compose swap() with Gate.send()/claim() WITHOUT changing this contract
///         (swap() already takes an explicit `to` recipient).
contract SwapPool is ReentrancyGuard {
    using SafeERC20 for IERC20;

    /// @dev USD prices are fixed-point, scaled by PRICE_ONE (1e18). The stable's
    ///      price is exactly PRICE_ONE and can never be changed.
    uint256 public constant PRICE_ONE = 1e18;
    /// @dev basis-point denominator for the fee and the price-deviation guard.
    uint16 public constant BPS_DENOM = 10_000;

    struct TokenInfo {
        bool listed;
        uint8 decimals; // cached at listing time
        uint256 price; // USD price, PRICE_ONE-scaled
        uint256 reserve; // internal accounting — this IS the swap lock
    }

    // --- registry ---
    mapping(address token => TokenInfo) public tokens;
    /// @dev the core-price token; tokens[stable].price == PRICE_ONE forever.
    address public immutable stable;

    // --- governance / roles (mirrors Gate.sol) ---
    address public owner;
    address public pendingOwner;
    /// @dev the only role permitted to move prices (may equal owner, but is
    ///      separable so a low-trust price feeder cannot also move liquidity).
    address public oracle;

    // --- circuit breaker ---
    bool public paused;
    address public guardian;

    // --- economic parameters ---
    /// @dev swap fee in bps, charged on the USD value (accrues into reserves).
    uint16 public feeBps;
    /// @dev max allowed price move per setPrice() call, in bps (anti-fat-finger /
    ///      anti-compromise). Applies only to UPDATES (not the first price).
    uint16 public maxPriceDeviationBps;

    // --- events ---
    event TokenListed(address indexed token, uint256 price, uint8 decimals);
    event TokenDelisted(address indexed token);
    event PriceSet(address indexed token, uint256 oldPrice, uint256 newPrice);
    event LiquiditySeeded(address indexed token, uint256 amount, uint256 reserve);
    event LiquidityWithdrawn(address indexed token, uint256 amount, uint256 reserve, address to);
    event Swapped(
        address indexed sender,
        address indexed tokenIn,
        address indexed tokenOut,
        uint256 amountIn,
        uint256 amountOut,
        address to
    );
    event FeeSet(uint16 feeBps);
    event MaxPriceDeviationSet(uint16 bps);
    event OracleSet(address indexed oracle);
    // governance / pause (same shape as Gate.sol)
    event OwnershipTransferStarted(address indexed previousOwner, address indexed newOwner);
    event OwnershipTransferred(address indexed previousOwner, address indexed newOwner);
    event GuardianSet(address indexed guardian);
    event Paused(address indexed account);
    event Unpaused(address indexed account);

    // --- errors ---
    error NotOwner();
    error NotOracle();
    error ZeroAddress();
    error TokenNotListed(address token);
    error TokenAlreadyListed(address token);
    error SameToken();
    error ZeroAmount();
    error ZeroPrice();
    /// @dev the requested output exceeds the pool's lock for that token.
    error ExceedsLock(uint256 want, uint256 reserve);
    /// @dev computed output was below the caller's minimum (slippage / stale price).
    error Slippage(uint256 got, uint256 min);
    error StableRepriceForbidden();
    error PriceDeviationTooHigh(uint256 oldPrice, uint256 newPrice, uint16 maxBps);
    error ReserveNonZero();
    error FeeTooHigh();
    error DeviationTooHigh();
    error EnforcedPause();
    error NotAuthorizedToPause();

    modifier onlyOwner() {
        if (msg.sender != owner) revert NotOwner();
        _;
    }

    modifier onlyOracle() {
        if (msg.sender != oracle) revert NotOracle();
        _;
    }

    modifier whenNotPaused() {
        if (paused) revert EnforcedPause();
        _;
    }

    /// @param stable_             the core-price token (price pinned to PRICE_ONE)
    /// @param maxPriceDeviationBps_ initial per-update price cap in bps (e.g. 1000 = 10%)
    constructor(address stable_, uint16 maxPriceDeviationBps_) {
        if (stable_ == address(0)) revert ZeroAddress();
        if (maxPriceDeviationBps_ == 0 || maxPriceDeviationBps_ > BPS_DENOM) revert DeviationTooHigh();

        owner = msg.sender;
        oracle = msg.sender;
        emit OwnershipTransferred(address(0), msg.sender);
        emit OracleSet(msg.sender);

        maxPriceDeviationBps = maxPriceDeviationBps_;
        emit MaxPriceDeviationSet(maxPriceDeviationBps_);

        stable = stable_;
        uint8 dec = IERC20Metadata(stable_).decimals();
        tokens[stable_] = TokenInfo({listed: true, decimals: dec, price: PRICE_ONE, reserve: 0});
        emit TokenListed(stable_, PRICE_ONE, dec);
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

    function setOracle(address newOracle) external onlyOwner {
        if (newOracle == address(0)) revert ZeroAddress();
        oracle = newOracle;
        emit OracleSet(newOracle);
    }

    function setGuardian(address newGuardian) external onlyOwner {
        guardian = newGuardian;
        emit GuardianSet(newGuardian);
    }

    function setFee(uint16 newFeeBps) external onlyOwner {
        // cap the fee well below 100% so a swap can never round to a zero/negative
        // payout by fee alone (defensive; 1000 = 10%).
        if (newFeeBps > 1000) revert FeeTooHigh();
        feeBps = newFeeBps;
        emit FeeSet(newFeeBps);
    }

    function setMaxPriceDeviation(uint16 bps) external onlyOwner {
        if (bps == 0 || bps > BPS_DENOM) revert DeviationTooHigh();
        maxPriceDeviationBps = bps;
        emit MaxPriceDeviationSet(bps);
    }

    function pause() external {
        if (msg.sender != owner && msg.sender != guardian) revert NotAuthorizedToPause();
        if (!paused) {
            paused = true;
            emit Paused(msg.sender);
        }
    }

    function unpause() external onlyOwner {
        if (paused) {
            paused = false;
            emit Unpaused(msg.sender);
        }
    }

    // ---------------------------------------------------------------------
    // Token registry + pricing
    // ---------------------------------------------------------------------

    /// @notice List a token for swapping at an initial USD price (PRICE_ONE-scaled).
    /// @dev    Decimals are cached from IERC20Metadata. The stable is listed in the
    ///         constructor and cannot be re-listed.
    function listToken(address token, uint256 price) external onlyOwner {
        if (token == address(0)) revert ZeroAddress();
        if (tokens[token].listed) revert TokenAlreadyListed(token);
        if (price == 0) revert ZeroPrice();

        uint8 dec = IERC20Metadata(token).decimals();
        tokens[token] = TokenInfo({listed: true, decimals: dec, price: price, reserve: 0});
        emit TokenListed(token, price, dec);
    }

    /// @notice Update a token's pegged price (oracle-only), bounded by the
    ///         per-update deviation cap. The stable can never be repriced.
    function setPrice(address token, uint256 newPrice) external onlyOracle {
        TokenInfo storage t = tokens[token];
        if (!t.listed) revert TokenNotListed(token);
        if (token == stable) revert StableRepriceForbidden();
        if (newPrice == 0) revert ZeroPrice();

        uint256 oldPrice = t.price;
        // |new - old| / old <= maxDeviation
        uint256 diff = newPrice > oldPrice ? newPrice - oldPrice : oldPrice - newPrice;
        if (diff > Math.mulDiv(oldPrice, maxPriceDeviationBps, BPS_DENOM)) {
            revert PriceDeviationTooHigh(oldPrice, newPrice, maxPriceDeviationBps);
        }

        t.price = newPrice;
        emit PriceSet(token, oldPrice, newPrice);
    }

    /// @notice Delist a token. Only when its reserve is fully withdrawn, so we
    ///         never strand locked liquidity behind an unlisted entry.
    function delistToken(address token) external onlyOwner {
        TokenInfo storage t = tokens[token];
        if (!t.listed) revert TokenNotListed(token);
        if (t.reserve != 0) revert ReserveNonZero();
        delete tokens[token];
        emit TokenDelisted(token);
    }

    // ---------------------------------------------------------------------
    // Liquidity (protocol-owned)
    // ---------------------------------------------------------------------

    /// @notice Seed (or top up) a token's reserve — the lock that bounds swaps.
    /// @dev    Credits reserve by the ACTUAL amount received (fee-on-transfer safe).
    function seedLiquidity(address token, uint256 amount) external onlyOwner {
        TokenInfo storage t = tokens[token];
        if (!t.listed) revert TokenNotListed(token);
        if (amount == 0) revert ZeroAmount();

        uint256 before = IERC20(token).balanceOf(address(this));
        IERC20(token).safeTransferFrom(msg.sender, address(this), amount);
        uint256 received = IERC20(token).balanceOf(address(this)) - before;

        t.reserve += received;
        emit LiquiditySeeded(token, received, t.reserve);
    }

    /// @notice Withdraw reserve (rebalancing / decommission / fee capture).
    function withdrawLiquidity(address token, uint256 amount, address to) external onlyOwner {
        TokenInfo storage t = tokens[token];
        if (!t.listed) revert TokenNotListed(token);
        if (amount == 0) revert ZeroAmount();
        if (to == address(0)) revert ZeroAddress();
        if (amount > t.reserve) revert ExceedsLock(amount, t.reserve);

        t.reserve -= amount; // effects before interaction
        IERC20(token).safeTransfer(to, amount);
        emit LiquidityWithdrawn(token, amount, t.reserve, to);
    }

    // ---------------------------------------------------------------------
    // Swap
    // ---------------------------------------------------------------------

    /// @notice Price `amountIn` of `tokenIn` into `tokenOut` at the pegged rate
    ///         (net of fee). Pure pricing — does NOT check the reserve cap.
    function quote(address tokenIn, address tokenOut, uint256 amountIn)
        public
        view
        returns (uint256 amountOut)
    {
        TokenInfo storage ti = tokens[tokenIn];
        TokenInfo storage to = tokens[tokenOut];
        if (!ti.listed) revert TokenNotListed(tokenIn);
        if (!to.listed) revert TokenNotListed(tokenOut);
        return _amountOut(amountIn, ti.price, ti.decimals, to.price, to.decimals, feeBps);
    }

    /// @notice Swap `amountIn` of `tokenIn` for `tokenOut`, capped by the output
    ///         token's locked reserve. Sends the output to `to`.
    /// @param minAmountOut caller's slippage / stale-price floor
    function swap(address tokenIn, address tokenOut, uint256 amountIn, uint256 minAmountOut, address to)
        external
        whenNotPaused
        nonReentrant
        returns (uint256 amountOut)
    {
        if (tokenIn == tokenOut) revert SameToken();
        if (amountIn == 0) revert ZeroAmount();
        if (to == address(0)) revert ZeroAddress();

        TokenInfo storage ti = tokens[tokenIn];
        TokenInfo storage tOut = tokens[tokenOut];
        if (!ti.listed) revert TokenNotListed(tokenIn);
        if (!tOut.listed) revert TokenNotListed(tokenOut);

        // Pull first (fee-on-transfer safe) so pricing uses the amount actually
        // received. This is the only external call before we compute + book.
        uint256 balBefore = IERC20(tokenIn).balanceOf(address(this));
        IERC20(tokenIn).safeTransferFrom(msg.sender, address(this), amountIn);
        uint256 received = IERC20(tokenIn).balanceOf(address(this)) - balBefore;
        if (received == 0) revert ZeroAmount();

        amountOut = _amountOut(received, ti.price, ti.decimals, tOut.price, tOut.decimals, feeBps);
        if (amountOut == 0) revert ZeroAmount();
        if (amountOut < minAmountOut) revert Slippage(amountOut, minAmountOut);
        // THE LOCK: never pay out more than this token's reserve.
        if (amountOut > tOut.reserve) revert ExceedsLock(amountOut, tOut.reserve);

        // Effects before the outgoing interaction (CEI). The fee stays in the
        // pool as retained reserve value (reserveIn grows by the full input,
        // reserveOut shrinks by only the net output).
        ti.reserve += received;
        tOut.reserve -= amountOut;
        emit Swapped(msg.sender, tokenIn, tokenOut, received, amountOut, to);

        IERC20(tokenOut).safeTransfer(to, amountOut);
    }

    // ---------------------------------------------------------------------
    // Internal pricing
    // ---------------------------------------------------------------------

    /// @dev out = amountIn * priceIn * 10^decOut / (priceOut * 10^decIn), less fee.
    ///      Uses mulDiv (512-bit intermediate) to avoid overflow, and floors —
    ///      every rounding step favors the pool.
    function _amountOut(
        uint256 amountIn,
        uint256 priceIn,
        uint8 decIn,
        uint256 priceOut,
        uint8 decOut,
        uint16 fee
    ) internal pure returns (uint256) {
        // USD value of the input, PRICE_ONE-scaled.
        uint256 usd = Math.mulDiv(amountIn, priceIn, 10 ** decIn);
        if (fee != 0) {
            // fee rounds UP against the user (mulDiv ceil), pool keeps the dust.
            uint256 feeUsd = Math.mulDiv(usd, fee, BPS_DENOM, Math.Rounding.Ceil);
            usd = usd - feeUsd;
        }
        // Convert USD back into output-token units (floors).
        return Math.mulDiv(usd, 10 ** decOut, priceOut);
    }

    /// @notice Convenience view: the current maximum swap OUTPUT for a token
    ///         (its reserve) and that value in USD (PRICE_ONE-scaled).
    function maxSwapOut(address token) external view returns (uint256 reserve, uint256 usdValue) {
        TokenInfo storage t = tokens[token];
        if (!t.listed) revert TokenNotListed(token);
        reserve = t.reserve;
        usdValue = Math.mulDiv(reserve, t.price, 10 ** t.decimals);
    }
}
