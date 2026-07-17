// SPDX-License-Identifier: MIT
pragma solidity 0.8.24;

import {Script, console2} from "forge-std/Script.sol";
import {ERC20} from "@openzeppelin/contracts/token/ERC20/ERC20.sol";
import {SwapPool} from "../src/SwapPool.sol";
import {Gate} from "../src/Gate.sol";
import {SwapRouter} from "../src/SwapRouter.sol";

/// @dev Mintable ERC-20 with configurable decimals.
contract XMintable is ERC20 {
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

/// @notice Deploy ONE chain's cross-chain-swap stack: a 6-dec stable hub, one
///         18-dec alt token, a seeded SwapPool, a Gate (single validator), and a
///         SwapRouter wired to both. Writes fixtures/xswap-<chainid>.env for
///         scripts/xswap.sh, which cross-wires the two chains' routers.
///
///         Env in: VALIDATOR (address), ALT_PRICE (uint, PRICE_ONE-scaled),
///                 ALT_SYMBOL (string).
contract DeployXSwap is Script {
    uint16 constant DEVIATION_BPS = 1000;

    function run() external {
        address validator = vm.envAddress("VALIDATOR");
        uint256 altPrice = vm.envUint("ALT_PRICE");
        string memory altSym = vm.envString("ALT_SYMBOL");

        vm.startBroadcast();

        XMintable usd = new XMintable("USD", "USD", 6);
        XMintable alt = new XMintable(altSym, altSym, 18);

        SwapPool pool = new SwapPool(address(usd), DEVIATION_BPS);
        pool.listToken(address(alt), altPrice);
        _seed(pool, usd, 10_000_000e6);
        _seed(pool, alt, 1_000_000e18);

        address[] memory vals = new address[](1);
        vals[0] = validator;
        Gate gate = new Gate(vals, 1);

        SwapRouter router = new SwapRouter(gate, pool);

        vm.stopBroadcast();

        string memory env = string.concat(
            "STABLE=", vm.toString(address(usd)), "\n",
            "ALT=", vm.toString(address(alt)), "\n",
            "POOL=", vm.toString(address(pool)), "\n",
            "GATE=", vm.toString(address(gate)), "\n",
            "ROUTER=", vm.toString(address(router)), "\n"
        );
        vm.writeFile(string.concat("fixtures/xswap-", vm.toString(block.chainid), ".env"), env);

        console2.log("chain   :", block.chainid);
        console2.log("stable  :", address(usd));
        console2.log("alt     :", address(alt));
        console2.log("pool    :", address(pool));
        console2.log("gate    :", address(gate));
        console2.log("router  :", address(router));
    }

    function _seed(SwapPool pool, XMintable token, uint256 amt) internal {
        token.mint(msg.sender, amt);
        token.approve(address(pool), amt);
        pool.seedLiquidity(address(token), amt);
    }
}
