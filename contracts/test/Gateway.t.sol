// SPDX-License-Identifier: Apache-2.0
pragma solidity 0.8.34;

import {Test} from "forge-std/Test.sol";
import {Vm} from "forge-std/Vm.sol";
import {BeefyClient} from "../src/BeefyClient.sol";
import {Gateway} from "../src/Gateway.sol";
import {ScaleCodec} from "../src/utils/ScaleCodec.sol";
import {IGateway, OutboundMessage} from "../src/interfaces/IGateway.sol";

contract GatewayTest is Test {
    Gateway gateway;
    BeefyClient client;

    function setUp() public {
        BeefyClient.ValidatorSet memory initial =
            BeefyClient.ValidatorSet({id: 0, length: 4, root: bytes32(uint256(1))});
        BeefyClient.ValidatorSet memory next =
            BeefyClient.ValidatorSet({id: 1, length: 4, root: bytes32(uint256(2))});
        client = new BeefyClient(8, 100, 3, 3, 0, initial, next);
        gateway = new Gateway(client);
    }

    /* ── sendMessage / outbound ──────────────────────────────────────── */

    function test_sendMessageIncrementsNonceAndEmits() public {
        bytes memory payload = hex"deadbeef";

        vm.expectEmit(true, true, false, true);
        emit IGateway.OutboundMessageAccepted(1, address(this), payload);
        assertEq(gateway.sendMessage(payload), 1);

        vm.expectEmit(true, true, false, true);
        emit IGateway.OutboundMessageAccepted(2, address(this), payload);
        assertEq(gateway.sendMessage(payload), 2);

        assertEq(gateway.outboundNonce(), 2);
    }

    /* ── hashMessageLeaf SCALE encoding ──────────────────────────────── */

    /// @notice Cross-checks {hashMessageLeaf} against a manually constructed
    ///         SCALE encoding for a known small message. If this test ever
    ///         diverges from what `pallet-bridge-outbound` produces, the
    ///         Substrate-side Merkle proof will not verify.
    function test_hashMessageLeafMatchesScaleEncoding() public view {
        OutboundMessage memory m = OutboundMessage({
            nonce: 1,
            destination: address(0xcafE000000000000000000000000000000000001),
            payload: hex"abcd"
        });

        // SCALE: u64 LE (8) + 20-byte address + compact u32 length (1 byte for <64) + payload
        bytes memory expected = bytes.concat(
            hex"0100000000000000",                              // nonce = 1 LE
            hex"cafe000000000000000000000000000000000001",      // destination
            hex"08",                                             // compact(2) = (2 << 2) | 0 = 0x08
            hex"abcd"
        );
        bytes32 expectedHash = keccak256(expected);
        assertEq(_call_hashMessageLeaf(m), expectedHash);
    }

    function test_hashMessageLeafCompactBoundary() public view {
        // Compact-encoded length crosses the 1-byte → 2-byte boundary at 64.
        bytes memory payload = new bytes(64);
        OutboundMessage memory m =
            OutboundMessage({nonce: 7, destination: address(0xBEEF), payload: payload});

        // compact(64) = 0x01_01 little-endian → 0x01 0x01
        bytes memory expected = bytes.concat(
            hex"0700000000000000",
            hex"000000000000000000000000000000000000beef",
            hex"0101",
            payload
        );
        assertEq(_call_hashMessageLeaf(m), keccak256(expected));
    }

    /* ── revert paths on submitInbound ───────────────────────────────── */

    function test_submitInbound_revertsOnUnknownMmrRoot() public {
        OutboundMessage memory m =
            OutboundMessage({nonce: 1, destination: address(0xBEEF), payload: hex""});
        Gateway.MmrLeaf memory leaf;
        leaf.version = 0;
        bytes32[] memory siblings;
        bytes32[] memory msgSiblings;
        Gateway.MmrLeafProof memory leafProof = Gateway.MmrLeafProof({siblings: siblings, order: 0});
        Gateway.MessageProof memory msgProof =
            Gateway.MessageProof({position: 0, width: 1, proof: msgSiblings});

        // BeefyClient.latestMMRRoot is zero; verifyMMRLeafProof returns false
        // because the leaf hash will not equal zero. Gateway should revert
        // with InvalidMmrLeafProof.
        vm.expectRevert(Gateway.InvalidMmrLeafProof.selector);
        gateway.submitInbound(m, leaf, leafProof, msgProof);
    }

    /// Solidity won't accept `calldata` for in-test struct construction, so we
    /// trampoline through `this.` to convert memory → calldata.
    function _call_hashMessageLeaf(OutboundMessage memory m) internal view returns (bytes32) {
        return this.callHashMessageLeaf(m);
    }

    function callHashMessageLeaf(OutboundMessage calldata m) external view returns (bytes32) {
        return gateway.hashMessageLeaf(m);
    }
}
