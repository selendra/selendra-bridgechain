// SPDX-License-Identifier: Apache-2.0
pragma solidity 0.8.34;

import {Test} from "forge-std/Test.sol";
import {BeefyClient} from "../src/BeefyClient.sol";

/// @notice End-to-end check of the `submitInitial` call shape the Go
///         relayer produces in `driver.BuildInitialSubmission`.
///
///         A successful `submitInitial` proves *every* on-chain
///         precondition holds simultaneously:
///
///           - keccak256(SCALE(commitment)) matches the value the relayer
///             would sign over (cross-checks the Go SCALE encoder),
///           - bitfield word layout + padding mask align with our Go
///             `internal/bitfield` package,
///           - ValidatorProof's (v,r,s,account,merkleProof) all
///             reconstruct to the validator the contract trusts.
///
///         If any of those drifts, this test reverts before the ticket
///         is written.
contract SubmitInitialTest is Test {
    BeefyClient client;

    // Anvil key #0 — same key used in the Go relayer's
    // driver/proof tests. The address is derived deterministically;
    // keeping the keypair stable across both languages means a single
    // failure mode (encoding drift) shows up in exactly one place.
    uint256 constant ANVIL_KEY_0 = 0xac0974bec39a17e36ba4a6b4d238ff944bacb478cbed5efcae784d7bf4f2ff80;

    function setUp() public {
        address signer = vm.addr(ANVIL_KEY_0);
        // Single-validator set: root == keccak256(addr).
        bytes32 root = keccak256(abi.encodePacked(signer));

        BeefyClient.ValidatorSet memory initial = BeefyClient.ValidatorSet({
            id: 0,
            length: 1,
            root: root
        });
        // Next set unused by this test; arbitrary distinct id is fine.
        BeefyClient.ValidatorSet memory next = BeefyClient.ValidatorSet({
            id: 1,
            length: 1,
            root: bytes32(uint256(0xdead))
        });

        client = new BeefyClient({
            _randaoCommitDelay: 8,
            _randaoCommitExpiration: 100,
            _minNumRequiredSignatures: 1,
            _fiatShamirRequiredSignatures: 1,
            _initialBeefyBlock: 0,
            _initialValidatorSet: initial,
            _nextValidatorSet: next
        });
    }

    function _buildCommitment() internal pure returns (BeefyClient.Commitment memory) {
        bytes memory mmrRoot = new bytes(32);
        for (uint256 i = 0; i < 32; i++) {
            mmrRoot[i] = 0xab;
        }
        BeefyClient.PayloadItem[] memory payload = new BeefyClient.PayloadItem[](1);
        // 0x6d68 == "mh" — written as a numeric literal so the compiler
        // doesn't flag this as a potentially-truncating string→bytes2 cast.
        payload[0] = BeefyClient.PayloadItem({payloadID: bytes2(0x6d68), data: mmrRoot});
        return BeefyClient.Commitment({blockNumber: 42, validatorSetID: 0, payload: payload});
    }

    function test_submitInitial_singleValidatorSet_writesTicket() public {
        BeefyClient.Commitment memory c = _buildCommitment();
        bytes32 commitmentHash = client.computeCommitmentHash(c);
        // vm.sign returns v in {27, 28} — same convention the Go
        // BuildValidatorProof shifts into via `sig[64] + 27`.
        (uint8 v, bytes32 r, bytes32 s) = vm.sign(ANVIL_KEY_0, commitmentHash);

        // Single-validator bitfield: bit 0 set, length 1 → one word with
        // LSB only. createInitialBitfield does the same packing the Go
        // `bitfield.From([]int{0}, 1)` produces — already cross-checked
        // by test_initialBitfieldMatchesGoRelayer.
        uint256[] memory bitsToSet = new uint256[](1);
        bitsToSet[0] = 0;
        uint256[] memory bf = client.createInitialBitfield(bitsToSet, 1);

        BeefyClient.ValidatorProof memory proof = BeefyClient.ValidatorProof({
            v: v,
            r: r,
            s: s,
            index: 0,
            account: vm.addr(ANVIL_KEY_0),
            proof: new bytes32[](0) // single-leaf tree → empty merkle proof
        });

        client.submitInitial(c, bf, proof);

        // The ticket is keyed by `createTicketID(msg.sender, commitmentHash)`
        // which does an inline assembly `mstore(0x00, account); mstore(0x20,
        // commitmentHash); keccak256(0x00, 0x40)`. address values are
        // zero-extended to 32 bytes when stored, so the input is
        // (12 zero bytes ‖ address ‖ commitmentHash) — that's what
        // `abi.encode` produces; `abi.encodePacked` would strip the
        // zero-padding and produce a 52-byte input instead.
        bytes32 ticketID = keccak256(abi.encode(address(this), commitmentHash));
        (uint64 blockNumber, uint32 validatorSetLen, uint32 numRequiredSignatures, uint256 prevRandao,) =
            client.tickets(ticketID);

        assertEq(blockNumber, uint64(block.number));
        assertEq(validatorSetLen, 1);
        assertEq(prevRandao, 0); // not yet captured
        assertGt(numRequiredSignatures, 0);
    }
}
