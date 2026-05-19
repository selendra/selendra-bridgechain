// SPDX-License-Identifier: Apache-2.0
pragma solidity 0.8.34;

import {BeefyClient} from "./BeefyClient.sol";
import {MMRProof} from "./utils/MMRProof.sol";
import {ScaleCodec} from "./utils/ScaleCodec.sol";
import {SubstrateMerkleProof} from "./utils/SubstrateMerkleProof.sol";
import {IGateway, OutboundMessage, InboundMessage} from "./interfaces/IGateway.sol";

/**
 * @title Gateway
 * @notice Bridge endpoint between Ethereum and the bridgechain Substrate
 *         solochain.
 *
 *         ## Substrate → Ethereum (the cheap direction, BEEFY-backed)
 *
 *         1. The bridgechain `bridge-outbound` pallet appends a leaf
 *            `keccak256(SCALE(Message))` into a per-block keccak Merkle tree
 *            on every `submit()`. The root of that tree is pinned into the
 *            BEEFY-MMR leaf's `leaf_extra` field for that block.
 *         2. BEEFY validators sign the MMR root containing that leaf, and a
 *            relayer drives the BeefyClient through commit-reveal to store
 *            the verified MMR root on Ethereum.
 *         3. To deliver a single message, the relayer calls
 *            {submitInbound} on this contract with:
 *              - the message fields,
 *              - an MMR-leaf inclusion proof against the BeefyClient's
 *                current MMR root,
 *              - the BEEFY-MMR leaf body (so we can hash it to match what
 *                the MMR proof commits to), and
 *              - a per-block Merkle proof of the message under
 *                `leaf_extra`.
 *
 *         ## Ethereum → Substrate (no BEEFY)
 *
 *         `sendMessage` emits {OutboundMessageAccepted}. A relayer
 *         witnesses the log on a finalized Ethereum block, hands it to the
 *         `bridge-inbound` pallet, which verifies via its beacon client.
 *
 *         ## Alignment notes (READ ME)
 *
 *         This MVP assumes the BEEFY-MMR leaf's `leaf_extra` is a flat
 *         `bytes32` (the keccak root over messages). The current bridgechain
 *         outbound pallet types it as `Vec<u8>`, which SCALE-encodes with a
 *         compact length prefix and so will not match {hashMmrLeaf} below.
 *
 *         Pick one before integration:
 *
 *           a) On bridgechain: change `BeefyDataProvider<Vec<u8>>` to
 *              `BeefyDataProvider<H256>` and `pallet_beefy_mmr::Config::
 *              LeafExtra = H256`. Recommended — cleanest match for
 *              snowbridge's leaf wire format.
 *
 *           b) Here: extend {hashMmrLeaf} to compact-encode the leaf_extra
 *              length before the bytes. Self-contained but means every
 *              consumer of the MMR leaf shape has to mirror it.
 *
 *         The per-message leaf is `keccak256(SCALE(nonce ++ destination ++
 *         payload))` and {hashMessageLeaf} implements that encoding inline.
 */
contract Gateway is IGateway {
    /// @dev The BEEFY light client this Gateway trusts.
    BeefyClient public immutable beefyClient;

    /// @dev Strictly-increasing outbound nonce. Read by relayers as part of
    ///      the OutboundMessageAccepted event; not used for replay protection
    ///      here because that's enforced on the Substrate side.
    uint64 public outboundNonce;

    /// @dev Track delivered Substrate → Ethereum nonces to block replay.
    mapping(uint64 nonce => bool) public inboundDelivered;

    error InvalidMmrLeafProof();
    error InvalidMessageProof();
    error NonceAlreadyDelivered();

    constructor(BeefyClient _beefyClient) {
        beefyClient = _beefyClient;
    }

    /* ── Ethereum → Substrate ─────────────────────────────────────────── */

    function sendMessage(bytes calldata payload) external returns (uint64 nonce) {
        unchecked {
            outboundNonce += 1;
        }
        nonce = outboundNonce;
        emit OutboundMessageAccepted(nonce, msg.sender, payload);
    }

    /* ── Substrate → Ethereum ─────────────────────────────────────────── */

    /// @notice MMR leaf shape emitted by `pallet-beefy-mmr` on bridgechain.
    ///         `leafExtra` is the per-block message commitment root.
    struct MmrLeaf {
        uint8 version;
        uint32 parentNumber;
        bytes32 parentHash;
        uint64 nextAuthoritySetID;
        uint32 nextAuthoritySetLen;
        bytes32 nextAuthoritySetRoot;
        bytes32 leafExtra;
    }

    /// @notice Substrate-side per-block Merkle inclusion of a single message.
    ///         `position` is the leaf index, `width` the leaf count.
    struct MessageProof {
        uint256 position;
        uint256 width;
        bytes32[] proof;
    }

    /// @notice MMR inclusion proof of the BEEFY-MMR leaf containing
    ///         `leafExtra` in the BeefyClient's currently trusted root.
    struct MmrLeafProof {
        bytes32[] siblings;
        uint256 order;
    }

    /**
     * @notice Verify and dispatch a Substrate-originated message.
     * @param message The decoded message struct.
     * @param leaf The full BEEFY-MMR leaf for the block where the message
     *        was queued. Hashed and verified against the MMR root.
     * @param leafProof MMR inclusion proof for `leaf`.
     * @param msgProof Per-block Merkle proof for the message under
     *        `leaf.leafExtra`.
     */
    function submitInbound(
        OutboundMessage calldata message,
        MmrLeaf calldata leaf,
        MmrLeafProof calldata leafProof,
        MessageProof calldata msgProof
    ) external {
        if (inboundDelivered[message.nonce]) {
            revert NonceAlreadyDelivered();
        }

        bytes32 leafHash = hashMmrLeaf(leaf);
        if (!beefyClient.verifyMMRLeafProof(leafHash, leafProof.siblings, leafProof.order)) {
            revert InvalidMmrLeafProof();
        }

        bytes32 messageLeaf = hashMessageLeaf(message);
        if (
            !SubstrateMerkleProof.verify(
                leaf.leafExtra, messageLeaf, msgProof.position, msgProof.width, msgProof.proof
            )
        ) {
            revert InvalidMessageProof();
        }

        inboundDelivered[message.nonce] = true;

        (bool ok,) = message.destination.call(message.payload);
        emit InboundMessageDispatched(message.nonce, message.destination, ok);
    }

    /* ── Encoding helpers ─────────────────────────────────────────────── */

    /**
     * @notice keccak256 of the SCALE encoding of a `bridge-outbound` Message.
     *         Wire format: nonce(u64 LE) ++ destination(20) ++
     *         compact_u32(payload.len) ++ payload.
     */
    function hashMessageLeaf(OutboundMessage calldata m) public pure returns (bytes32) {
        return keccak256(
            bytes.concat(
                ScaleCodec.encodeU64(m.nonce),
                bytes20(m.destination),
                ScaleCodec.checkedEncodeCompactU32(m.payload.length),
                m.payload
            )
        );
    }

    /**
     * @notice keccak256 of the SCALE encoding of the BEEFY-MMR leaf. Mirrors
     *         the layout produced by `pallet-beefy-mmr` once `leaf_extra` is
     *         typed as H256 (see contract NatSpec for the alignment note).
     */
    function hashMmrLeaf(MmrLeaf calldata leaf) public pure returns (bytes32) {
        return keccak256(
            bytes.concat(
                ScaleCodec.encodeU8(leaf.version),
                ScaleCodec.encodeU32(leaf.parentNumber),
                leaf.parentHash,
                ScaleCodec.encodeU64(leaf.nextAuthoritySetID),
                ScaleCodec.encodeU32(leaf.nextAuthoritySetLen),
                leaf.nextAuthoritySetRoot,
                leaf.leafExtra
            )
        );
    }
}
