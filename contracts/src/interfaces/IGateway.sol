// SPDX-License-Identifier: Apache-2.0
pragma solidity 0.8.34;

/**
 * @dev Outbound message envelope as the bridgechain `bridge-outbound` pallet
 *      emits it (post-alignment — see Gateway.sol header for the in-progress
 *      SCALE-vs-ABI encoding decision).
 */
struct OutboundMessage {
    /// Monotonic nonce assigned by the pallet on submit().
    uint64 nonce;
    /// Destination Ethereum contract.
    address destination;
    /// Opaque calldata for the destination, typically an ABI-encoded selector + args.
    bytes payload;
}

/**
 * @dev Inbound (Ethereum → Substrate) envelope. Emitted by the Gateway and
 *      consumed by the bridgechain `bridge-inbound` pallet via the relayer.
 */
struct InboundMessage {
    /// Caller-chosen identifier visible to the Substrate side; the pallet uses
    /// it for replay protection.
    uint64 nonce;
    /// Application origin on the Ethereum side (the contract that requested
    /// the bridge call). The Gateway sets this to msg.sender on send.
    address origin;
    /// Opaque payload to be dispatched by the inbound pallet's
    /// MessageDispatch hook.
    bytes payload;
}

/**
 * @title IGateway
 * @notice Public surface of the bridge Gateway. Used by applications calling
 *         out to Substrate and by relayers delivering Substrate messages.
 */
interface IGateway {
    /**
     * @notice Emitted on every successful outbound send. Relayers watch this
     *         log and feed it into the bridgechain inbound pallet.
     */
    event OutboundMessageAccepted(uint64 indexed nonce, address indexed origin, bytes payload);

    /**
     * @notice Emitted when an inbound (Substrate → Ethereum) message has been
     *         verified and dispatched.
     */
    event InboundMessageDispatched(uint64 indexed nonce, address indexed destination, bool success);

    /// @notice Send a message to the bridgechain side. Anyone may call.
    function sendMessage(bytes calldata payload) external returns (uint64 nonce);
}
