// SPDX-License-Identifier: Apache-2.0
pragma solidity 0.8.34;

import {Test} from "forge-std/Test.sol";
import {BeefyClient} from "../src/BeefyClient.sol";

/// @notice Sanity test: the BEEFY client deploys with sane init values.
///         The full commit-reveal flow is covered upstream in Snowfork's
///         test suite; these tests only check our wiring.
contract BeefyClientTest is Test {
    BeefyClient client;

    bytes32 constant INITIAL_SET_ROOT = bytes32(uint256(0x11));
    bytes32 constant NEXT_SET_ROOT = bytes32(uint256(0x22));

    function setUp() public {
        BeefyClient.ValidatorSet memory initial = BeefyClient.ValidatorSet({
            id: 0,
            length: 4,
            root: INITIAL_SET_ROOT
        });
        BeefyClient.ValidatorSet memory next = BeefyClient.ValidatorSet({
            id: 1,
            length: 4,
            root: NEXT_SET_ROOT
        });
        client = new BeefyClient({
            _randaoCommitDelay: 8,
            _randaoCommitExpiration: 100,
            _minNumRequiredSignatures: 3,
            _fiatShamirRequiredSignatures: 3,
            _initialBeefyBlock: 0,
            _initialValidatorSet: initial,
            _nextValidatorSet: next
        });
    }

    function test_deployedWithInitialState() public view {
        assertEq(client.latestBeefyBlock(), 0);
        assertEq(client.latestMMRRoot(), bytes32(0));

        (uint128 id, uint128 len, bytes32 root,) = client.currentValidatorSet();
        assertEq(id, 0);
        assertEq(len, 4);
        assertEq(root, INITIAL_SET_ROOT);

        (id, len, root,) = client.nextValidatorSet();
        assertEq(id, 1);
        assertEq(len, 4);
        assertEq(root, NEXT_SET_ROOT);
    }

    function test_revertsOnInvalidValidatorSetIds() public {
        BeefyClient.ValidatorSet memory initial = BeefyClient.ValidatorSet({
            id: 5,
            length: 4,
            root: INITIAL_SET_ROOT
        });
        BeefyClient.ValidatorSet memory next = BeefyClient.ValidatorSet({
            id: 7,
            length: 4,
            root: NEXT_SET_ROOT
        });

        vm.expectRevert("invalid-constructor-params");
        new BeefyClient(8, 100, 3, 3, 0, initial, next);
    }

    /// @notice Cross-checks `computeCommitmentHash` against the Go relayer's
    ///         `proof.CommitmentHash` for the same fixture (see
    ///         relayer/internal/proof/proof_test.go::referenceCommitment).
    ///         If these ever diverge the on-chain signature recovery for the
    ///         relayer's submitted ValidatorProofs will fail.
    /// @notice Cross-checks the Go relayer's `bitfield.From` against the
    ///         contract's own `createInitialBitfield`. Same input set →
    ///         same uint256[] words. If these ever drift, `submitInitial`
    ///         rejects with `InvalidBitfieldPadding` or wrong validators
    ///         are selected during the reveal phase.
    function test_initialBitfieldMatchesGoRelayer() public view {
        uint256[] memory bitsToSet = new uint256[](8);
        bitsToSet[0] = 0;
        bitsToSet[1] = 1;
        bitsToSet[2] = 7;
        bitsToSet[3] = 8;
        bitsToSet[4] = 255;
        bitsToSet[5] = 256;
        bitsToSet[6] = 257;
        bitsToSet[7] = 511;

        uint256[] memory bf = client.createInitialBitfield(bitsToSet, 512);
        assertEq(bf.length, 2);
        // Computed by Go fixture in relayer/internal/bitfield/bitfield_test.go.
        assertEq(bf[0], 0x8000000000000000000000000000000000000000000000000000000000000183);
        assertEq(bf[1], 0x8000000000000000000000000000000000000000000000000000000000000003);
    }

    function test_computeCommitmentHashMatchesGoRelayer() public view {
        bytes memory mmrRoot = new bytes(32);
        for (uint256 i = 0; i < 32; i++) {
            mmrRoot[i] = 0xab;
        }
        BeefyClient.PayloadItem[] memory payload = new BeefyClient.PayloadItem[](1);
        payload[0] = BeefyClient.PayloadItem({payloadID: bytes2("mh"), data: mmrRoot});
        BeefyClient.Commitment memory c = BeefyClient.Commitment({
            blockNumber: 42,
            validatorSetID: 7,
            payload: payload
        });

        // Value computed by the Go relayer against the same fixture in
        // proof_test.go::referenceCommitment.
        bytes32 expected = 0xed1608fc4c09deed8144d310da1aacc4cb3ab6b123ac1697374d52d720a5ead4;
        assertEq(client.computeCommitmentHash(c), expected);
    }
}
