// SPDX-License-Identifier: MIT
pragma solidity 0.8.24;

import {Test} from "forge-std/Test.sol";
import {ERC20} from "@openzeppelin/contracts/token/ERC20/ERC20.sol";
import {SwapPool} from "../src/SwapPool.sol";

/// @dev Mintable ERC-20 with configurable decimals, so a fixture can cover the
///      decimal normalisation that is the easy half of this math to get wrong.
contract FixToken is ERC20 {
    uint8 private immutable _dec;

    constructor(string memory n, uint8 d) ERC20(n, n) {
        _dec = d;
    }

    function decimals() public view override returns (uint8) {
        return _dec;
    }

    function mint(address to, uint256 a) external {
        _mint(to, a);
    }
}

/// @notice Generates swap-pricing fixtures shared with the Solana pool.
///         Run with: forge test --match-contract GenSwapMathFixtures
///         Writes fixtures/swap_math.json (inputs + the pool's own `quote`).
///
///         The Rust test (crates/solana-swap/tests/parity.rs) recomputes every
///         one and must agree to the unit. This is the swap-side equivalent of
///         the submissionId fixtures: the UI quotes with one implementation and
///         the chain executes with the other, so a one-unit disagreement is
///         either a reverting swap or a wrong payout.
contract GenSwapMathFixturesTest is Test {
    struct Case {
        string name;
        uint8 decIn;
        uint8 decOut;
        uint256 priceIn; // PRICE_ONE-scaled, quoted in hub units
        uint256 priceOut;
        uint16 feeBps;
        uint256 amountIn;
    }

    function test_writeFixtures() public {
        Case[8] memory cases = [
            Case("hub-to-token-9dp", 9, 9, 1e18, 3180e18, 0, 1_000_000_000_000),
            Case("token-to-hub-9dp", 9, 9, 3180e18, 1e18, 0, 1_000_000_000),
            Case("decimals-9-to-6", 9, 6, 3180e18, 1e18, 0, 1_000_000_000),
            Case("decimals-6-to-9", 6, 9, 1e18, 3180e18, 0, 3_180_000_000),
            Case("fee-30bps", 9, 9, 1e18, 2e18, 30, 1_000_000_000),
            Case("fee-1bps-dust", 0, 0, 1e18, 1e18, 1, 1),
            Case("cheap-token", 9, 9, 1e18, 2e18, 0, 500_000_000_000),
            Case("odd-price", 6, 9, 1_234_567_890_123_456_789, 987_654_321_098_765_432, 25, 1_000_000)
        ];

        string memory json = "{\"fixtures\":[";
        for (uint256 i = 0; i < cases.length; i++) {
            Case memory c = cases[i];
            uint256 got = _quote(c);
            json = string.concat(
                json,
                i == 0 ? "" : ",",
                "{\"name\":\"", c.name,
                "\",\"decIn\":", vm.toString(uint256(c.decIn)),
                ",\"decOut\":", vm.toString(uint256(c.decOut)),
                ",\"priceIn\":\"", vm.toString(c.priceIn),
                "\",\"priceOut\":\"", vm.toString(c.priceOut),
                "\",\"feeBps\":", vm.toString(uint256(c.feeBps)),
                ",\"amountIn\":\"", vm.toString(c.amountIn),
                "\",\"amountOut\":\"", vm.toString(got),
                "\"}"
            );
        }
        json = string.concat(json, "]}");
        vm.writeFile("fixtures/swap_math.json", json);
    }

    /// Quotes through a REAL pool rather than a copy of the formula — a fixture
    /// that re-implements the thing it is meant to pin proves nothing.
    function _quote(Case memory c) internal returns (uint256) {
        FixToken hub = new FixToken("HUB", c.decIn);
        FixToken other = new FixToken("OTHER", c.decOut);
        // Deviation cap is irrelevant here (no reprice), fee is what we vary.
        SwapPool pool = new SwapPool(address(hub), 1000);
        pool.setFee(c.feeBps);
        // `hub` is listed by the constructor at 1.0, so drive both sides through
        // the generic path by listing `other` and quoting through a third token
        // when priceIn != 1.0.
        pool.listToken(address(other), c.priceOut);
        if (c.priceIn == 1e18) {
            return pool.quote(address(hub), address(other), c.amountIn);
        }
        FixToken input = new FixToken("IN", c.decIn);
        pool.listToken(address(input), c.priceIn);
        return pool.quote(address(input), address(other), c.amountIn);
    }
}
