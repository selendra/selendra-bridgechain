// SPDX-License-Identifier: MIT
pragma solidity 0.8.24;

import {Test} from "forge-std/Test.sol";
import {BridgeHash} from "../src/BridgeHash.sol";

/// @notice Generates submissionId fixtures shared with the Rust side (Phase 3).
///         Run with: forge test --match-contract GenFixtures
///         Writes fixtures/submission_ids.json (inputs + Solidity-computed ids).
///         The Rust test (bridge-core) reads this file and must recompute every id.
contract GenFixturesTest is Test {
    struct F {
        string name;
        bytes32 debridgeId;
        uint256 amount;
        uint256 chainIdFrom;
        uint256 chainIdTo;
        uint256 nonce;
        bytes receiver;
        bool hasAuto;
        uint256 executionFee;
        uint256 flags;
        bytes fallbackAddress;
        bytes data;
        bytes nativeSender;
    }

    function _id(F memory f) internal pure returns (bytes32) {
        if (!f.hasAuto) {
            return BridgeHash.getSubmissionId(
                f.debridgeId, f.amount, f.chainIdFrom, f.chainIdTo, f.nonce, f.receiver
            );
        }
        return BridgeHash.getSubmissionIdWithAuto(
            f.debridgeId,
            f.amount,
            f.chainIdFrom,
            f.chainIdTo,
            f.nonce,
            f.receiver,
            BridgeHash.AutoParams({
                executionFee: f.executionFee,
                flags: f.flags,
                fallbackAddress: f.fallbackAddress,
                data: f.data,
                nativeSender: f.nativeSender
            })
        );
    }

    function _obj(F memory f) internal pure returns (string memory) {
        return string.concat(
            "{",
            '"name":"', f.name, '",',
            '"debridgeId":"', vm.toString(f.debridgeId), '",',
            '"amount":"', vm.toString(f.amount), '",',
            '"chainIdFrom":', vm.toString(f.chainIdFrom), ",",
            '"chainIdTo":', vm.toString(f.chainIdTo), ",",
            '"nonce":', vm.toString(f.nonce), ",",
            '"receiver":"', vm.toString(f.receiver), '",',
            '"hasAuto":', f.hasAuto ? "true" : "false", ",",
            '"executionFee":"', vm.toString(f.executionFee), '",',
            '"flags":"', vm.toString(f.flags), '",',
            '"fallbackAddress":"', vm.toString(f.fallbackAddress), '",',
            '"data":"', vm.toString(f.data), '",',
            '"nativeSender":"', vm.toString(f.nativeSender), '",',
            '"submissionId":"', vm.toString(_id(f)), '"',
            "}"
        );
    }

    function test_WriteFixtures() public {
        F[] memory fs = new F[](3);

        // 1) plain transfer, no execution payload, EVM 20-byte receiver
        fs[0] = F({
            name: "no-auto",
            debridgeId: BridgeHash.getDebridgeId(1337, address(0x1234)),
            amount: 100 ether,
            chainIdFrom: 1337,
            chainIdTo: 1338,
            nonce: 0,
            receiver: abi.encodePacked(address(0xCAFE)),
            hasAuto: false,
            executionFee: 0,
            flags: 0,
            fallbackAddress: "",
            data: "",
            nativeSender: ""
        });

        // 2) transfer WITH an execution payload
        fs[1] = F({
            name: "with-auto",
            debridgeId: BridgeHash.getDebridgeId(1337, address(0xABCD)),
            amount: 5_000_000,
            chainIdFrom: 1337,
            chainIdTo: 56,
            nonce: 7,
            receiver: abi.encodePacked(address(0xBEEF)),
            hasAuto: true,
            executionFee: 1 ether,
            flags: 2,
            fallbackAddress: abi.encodePacked(address(0xF00D)),
            data: hex"deadbeef0102",
            nativeSender: abi.encodePacked(address(0x5151))
        });

        // 3) non-EVM-shaped 32-byte receiver, large nonce/amount, no auto
        fs[2] = F({
            name: "long-receiver",
            debridgeId: BridgeHash.getDebridgeId(10, address(0x9999)),
            amount: type(uint256).max,
            chainIdFrom: 10,
            chainIdTo: 7565164, // Solana-style large chain id
            nonce: 123456789,
            receiver: hex"00112233445566778899aabbccddeeff00112233445566778899aabbccddeeff",
            hasAuto: false,
            executionFee: 0,
            flags: 0,
            fallbackAddress: "",
            data: "",
            nativeSender: ""
        });

        string memory json = "{\"fixtures\":[";
        for (uint256 i = 0; i < fs.length; i++) {
            json = string.concat(json, _obj(fs[i]));
            if (i + 1 < fs.length) json = string.concat(json, ",");
        }
        json = string.concat(json, "]}");

        vm.writeFile("fixtures/submission_ids.json", json);

        // sanity: log the ids
        for (uint256 i = 0; i < fs.length; i++) {
            emit log_named_bytes32(fs[i].name, _id(fs[i]));
        }
    }
}
