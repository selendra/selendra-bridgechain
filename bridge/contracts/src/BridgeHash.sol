// SPDX-License-Identifier: MIT
pragma solidity 0.8.24;

/// @title BridgeHash
/// @notice Canonical, deterministic hashing for cross-chain messages.
/// @dev    Mirrors deBridge's scheme (debridge-contracts-v1 DeBridgeGate):
///         everything is `abi.encodePacked` so it can be reproduced byte-for-byte
///         off-chain with alloy's `abi_encode_packed` in the Rust validator/keeper.
///         THE HASH IS SACRED — Solidity and Rust must agree (Phase 3 test).
library BridgeHash {
    /// @dev domain-separating prefix, as in deBridge.
    uint256 internal constant SUBMISSION_PREFIX = 1;

    /// @notice Optional execution payload attached to a transfer.
    /// @dev    `nativeSender` is meaningful only on the claim (From) side; on the
    ///         source side it is the packed msg.sender that the gate injects.
    struct AutoParams {
        uint256 executionFee;
        uint256 flags;
        bytes fallbackAddress;
        bytes data;
        bytes nativeSender;
    }

    /// @notice Asset id = keccak256(abi.encodePacked(nativeChainId, nativeToken)).
    function getDebridgeId(uint256 nativeChainId, address nativeToken)
        internal
        pure
        returns (bytes32)
    {
        return keccak256(abi.encodePacked(nativeChainId, nativeToken));
    }

    /// @dev The 7-field base of every submissionId, returned unhashed so the
    ///      auto-variant can append to it (exactly as deBridge does).
    function packedSubmission(
        bytes32 debridgeId,
        uint256 amount,
        uint256 chainIdFrom,
        uint256 chainIdTo,
        uint256 nonce,
        bytes memory receiver
    ) internal pure returns (bytes memory) {
        return abi.encodePacked(
            SUBMISSION_PREFIX, debridgeId, chainIdFrom, chainIdTo, amount, receiver, nonce
        );
    }

    /// @notice submissionId for a transfer WITHOUT an execution payload.
    function getSubmissionId(
        bytes32 debridgeId,
        uint256 amount,
        uint256 chainIdFrom,
        uint256 chainIdTo,
        uint256 nonce,
        bytes memory receiver
    ) internal pure returns (bytes32) {
        return keccak256(
            packedSubmission(debridgeId, amount, chainIdFrom, chainIdTo, nonce, receiver)
        );
    }

    /// @notice submissionId for a transfer WITH an execution payload.
    function getSubmissionIdWithAuto(
        bytes32 debridgeId,
        uint256 amount,
        uint256 chainIdFrom,
        uint256 chainIdTo,
        uint256 nonce,
        bytes memory receiver,
        AutoParams memory autoParams
    ) internal pure returns (bytes32) {
        return keccak256(
            abi.encodePacked(
                packedSubmission(debridgeId, amount, chainIdFrom, chainIdTo, nonce, receiver),
                autoParams.executionFee,
                autoParams.flags,
                keccak256(autoParams.fallbackAddress),
                keccak256(autoParams.data),
                keccak256(autoParams.nativeSender)
            )
        );
    }
}
