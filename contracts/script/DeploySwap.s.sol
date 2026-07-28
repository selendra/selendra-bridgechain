// SPDX-License-Identifier: MIT
pragma solidity 0.8.24;

import {Script, console2} from "forge-std/Script.sol";
import {ERC20} from "@openzeppelin/contracts/token/ERC20/ERC20.sol";
import {SwapPool} from "../src/SwapPool.sol";

/// @dev Mintable ERC-20 with configurable decimals, for local swap demos.
contract Mintable is ERC20 {
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

/// @notice Deploy a same-chain SwapPool with a 6-decimal stablecoin hub and two
///         listed tokens (18-dec), seed their reserves, and write the addresses
///         to fixtures/swap-deploy.env for scripts/swap.sh to consume.
contract DeploySwap is Script {
    // pegged prices, PRICE_ONE (1e18)-scaled
    uint256 constant WETH_PRICE = 3180e18;
    uint256 constant TT_PRICE = 1e18;
    uint16 constant DEVIATION_BPS = 1000; // 10% per-update cap

    struct Deployed {
        address pool;
        address usd;
        address weth;
        address tt;
    }

    function run() external {
        Deployed memory d = _deploy();
        _record(d);
    }

    function _deploy() internal returns (Deployed memory d) {
        vm.startBroadcast();

        Mintable usd = new Mintable("Mock USD", "mUSD", 6);
        Mintable weth = new Mintable("Wrapped Ether", "WETH", 18);
        Mintable tt = new Mintable("Test Token", "TT", 18);

        SwapPool pool = new SwapPool(address(usd), DEVIATION_BPS);
        pool.listToken(address(weth), WETH_PRICE);
        pool.listToken(address(tt), TT_PRICE);

        // Seed reserves (the swap locks).
        _seed(pool, usd, 1_000_000e6);
        _seed(pool, weth, 100e18);
        _seed(pool, tt, 500_000e18);

        vm.stopBroadcast();

        d = Deployed(address(pool), address(usd), address(weth), address(tt));
    }

    function _record(Deployed memory d) internal {
        string memory env = string.concat(
            "SWAP_POOL=", vm.toString(d.pool), "\n",
            "STABLE=", vm.toString(d.usd), "\n",
            "WETH=", vm.toString(d.weth), "\n",
            "TT=", vm.toString(d.tt), "\n"
        );
        vm.writeFile("fixtures/swap-deploy.env", env);
        console2.log("SwapPool:", d.pool);
        console2.log("stable  :", d.usd);
        console2.log("WETH    :", d.weth);
        console2.log("TT      :", d.tt);
    }

    function _seed(SwapPool pool, Mintable token, uint256 amount) internal {
        token.mint(msg.sender, amount);
        token.approve(address(pool), amount);
        pool.seedLiquidity(address(token), amount);
    }
}
