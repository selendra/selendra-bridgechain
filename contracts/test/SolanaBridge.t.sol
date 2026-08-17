// SPDX-License-Identifier: MIT
pragma solidity 0.8.24;

import {Test} from "forge-std/Test.sol";
import {Gate} from "../src/Gate.sol";
import {deployTestGate, TEST_BRIDGE_DOMAIN} from "./helpers/TestGate.sol";
import {TestToken} from "../src/TestToken.sol";
import {BridgeHash} from "../src/BridgeHash.sol";

/// @notice EVM -> Solana send path (Phase 8).
///
/// Solana account keys are 32 bytes, not 20. `send()` must accept a 32-byte
/// receiver so a transfer can target a Solana pubkey / SPL token account, while
/// still rejecting any other malformed width. The emitted `submissionId` is the
/// same sacred keccak hash the Solana gate program recomputes on the claim side.
contract SolanaBridgeTest is Test {
    Gate gate;
    TestToken token;

    address user = address(0xBEEF);

    /// deBridge's chain id for Solana mainnet — the same value used in the
    /// cross-language hash fixtures (contracts/fixtures/submission_ids.json).
    uint256 constant SOLANA_CHAIN_ID = 7565164;

    // A real-looking 32-byte Solana pubkey (base58 "Aaa…" decodes to 32 bytes).
    bytes constant SOLANA_RECEIVER =
        hex"00112233445566778899aabbccddeeff00112233445566778899aabbccddeeff";

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

    function setUp() public {
        address[] memory validators = new address[](1);
        validators[0] = address(0xA11CE);
        gate = deployTestGate(validators, 1);

        token = new TestToken("Test", "TST");
        token.mint(user, 1_000 ether);

        vm.prank(user);
        token.approve(address(gate), type(uint256).max);
    }

    function test_Send_ToSolana_EmitsExpectedSubmissionId() public {
        uint256 amount = 100 ether;
        bytes memory autoParams = "";
        bytes memory nativeSender = abi.encodePacked(user);

        bytes32 debridgeId = BridgeHash.getDebridgeId(block.chainid, address(token));
        bytes32 expectedId = BridgeHash.getSubmissionId(
            TEST_BRIDGE_DOMAIN, debridgeId, amount, block.chainid, SOLANA_CHAIN_ID, 0, SOLANA_RECEIVER
        );

        vm.expectEmit(true, true, true, true);
        emit Sent(
            expectedId,
            debridgeId,
            amount,
            block.chainid,
            SOLANA_CHAIN_ID,
            SOLANA_RECEIVER,
            0,
            autoParams,
            nativeSender,
            address(token)
        );

        vm.prank(user);
        bytes32 id = gate.send(address(token), amount, SOLANA_CHAIN_ID, SOLANA_RECEIVER, autoParams);

        assertEq(id, expectedId, "returned id mismatch");
    }

    function test_Send_ToSolana_LocksTokens() public {
        vm.prank(user);
        gate.send(address(token), 100 ether, SOLANA_CHAIN_ID, SOLANA_RECEIVER, "");

        assertEq(token.balanceOf(address(gate)), 100 ether, "gate did not hold funds");
        assertEq(token.balanceOf(user), 900 ether, "user not debited");
    }

    function test_Send_StillAccepts_20ByteEvmReceiver() public {
        bytes memory evmReceiver = abi.encodePacked(address(0xCAFE));
        vm.prank(user);
        gate.send(address(token), 1 ether, 1338, evmReceiver, "");
        // no revert == pass
    }

    function test_Send_Reverts_On_21ByteReceiver() public {
        bytes memory bad = new bytes(21);
        vm.prank(user);
        vm.expectRevert(Gate.BadReceiver.selector);
        gate.send(address(token), 1 ether, SOLANA_CHAIN_ID, bad, "");
    }

    function test_Send_Reverts_On_31ByteReceiver() public {
        bytes memory bad = new bytes(31);
        vm.prank(user);
        vm.expectRevert(Gate.BadReceiver.selector);
        gate.send(address(token), 1 ether, SOLANA_CHAIN_ID, bad, "");
    }

    function test_Send_Reverts_On_EmptyReceiver() public {
        vm.prank(user);
        vm.expectRevert(Gate.BadReceiver.selector);
        gate.send(address(token), 1 ether, SOLANA_CHAIN_ID, "", "");
    }
}
