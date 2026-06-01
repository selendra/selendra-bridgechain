// SPDX-License-Identifier: MIT
pragma solidity 0.8.24;

import {Test} from "forge-std/Test.sol";
import {Gate} from "../src/Gate.sol";
import {TestToken} from "../src/TestToken.sol";
import {BridgeHash} from "../src/BridgeHash.sol";

contract SendTest is Test {
    Gate gate;
    TestToken token;

    address user = address(0xBEEF);
    uint256 constant CHAIN_TO = 1338;

    event Sent(
        bytes32 indexed submissionId,
        bytes32 indexed debridgeId,
        uint256 amount,
        uint256 chainIdFrom,
        uint256 chainIdTo,
        bytes receiver,
        uint256 nonce,
        bytes autoParams,
        bytes nativeSender
    );

    function setUp() public {
        address[] memory validators = new address[](1);
        validators[0] = address(0xA11CE);
        gate = new Gate(validators, 1);

        token = new TestToken("Test", "TST");
        token.mint(user, 1_000 ether);

        vm.prank(user);
        token.approve(address(gate), type(uint256).max);
    }

    function test_Send_EmitsEventWithExpectedFields() public {
        uint256 amount = 100 ether;
        bytes memory receiver = abi.encodePacked(address(0xCAFE));
        bytes memory autoParams = "";
        bytes memory nativeSender = abi.encodePacked(user);

        bytes32 debridgeId = BridgeHash.getDebridgeId(block.chainid, address(token));
        bytes32 expectedId = BridgeHash.getSubmissionId(
            debridgeId, amount, block.chainid, CHAIN_TO, 0, receiver
        );

        vm.expectEmit(true, true, true, true);
        emit Sent(
            expectedId, debridgeId, amount, block.chainid, CHAIN_TO, receiver, 0, autoParams, nativeSender
        );

        vm.prank(user);
        bytes32 id = gate.send(address(token), amount, CHAIN_TO, receiver, autoParams);

        assertEq(id, expectedId, "returned id mismatch");
    }

    function test_Send_LocksTokensInGate() public {
        uint256 amount = 100 ether;
        bytes memory receiver = abi.encodePacked(address(0xCAFE));

        vm.prank(user);
        gate.send(address(token), amount, CHAIN_TO, receiver, "");

        assertEq(token.balanceOf(address(gate)), amount, "gate did not hold funds");
        assertEq(token.balanceOf(user), 900 ether, "user not debited");
    }

    function test_Send_NonceIncrementsPerTarget() public {
        bytes memory receiver = abi.encodePacked(address(0xCAFE));

        assertEq(gate.nonceTo(CHAIN_TO), 0);

        vm.prank(user);
        gate.send(address(token), 1 ether, CHAIN_TO, receiver, "");
        assertEq(gate.nonceTo(CHAIN_TO), 1, "nonce should be 1 after first send");

        vm.prank(user);
        gate.send(address(token), 1 ether, CHAIN_TO, receiver, "");
        assertEq(gate.nonceTo(CHAIN_TO), 2, "nonce should be 2 after second send");
    }

    function test_Send_NoncesAreIndependentPerTargetChain() public {
        bytes memory receiver = abi.encodePacked(address(0xCAFE));

        vm.prank(user);
        gate.send(address(token), 1 ether, 1338, receiver, "");
        vm.prank(user);
        gate.send(address(token), 1 ether, 9999, receiver, "");

        assertEq(gate.nonceTo(1338), 1);
        assertEq(gate.nonceTo(9999), 1);
    }

    function test_Send_RevertsOnZeroAmount() public {
        vm.prank(user);
        vm.expectRevert(Gate.ZeroAmount.selector);
        gate.send(address(token), 0, CHAIN_TO, abi.encodePacked(address(0xCAFE)), "");
    }
}
