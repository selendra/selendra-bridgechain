// SPDX-License-Identifier: MIT
pragma solidity 0.8.24;

import {ERC1967Proxy} from "@openzeppelin/contracts/proxy/ERC1967/ERC1967Proxy.sol";

/// @title GateProxy
/// @notice The contract that IS the bridge gate: an ERC1967 proxy delegating to a
///         {Gate} implementation.
///
/// @dev    A named subclass of `ERC1967Proxy` rather than the OZ contract used
///         directly, for one practical reason: deploy tooling addresses contracts
///         as `<source path>:<name>`, and a library contract reachable only
///         through a remapping is awkward to name on a `forge create` line. Giving
///         the proxy a file in `src/` makes the deploy command unambiguous and
///         makes the deployed bytecode identifiable on a block explorer as this
///         project's proxy rather than an anonymous one.
///
///         `data` MUST be the encoded `Gate.initialize(...)` call. The proxy
///         forwards it as a delegatecall from within this constructor, so the
///         gate is configured in the same transaction that creates it — there is
///         never a block in which an uninitialized proxy exists for someone else
///         to seize. Passing empty `data` would create exactly that hole, so
///         it is rejected.
contract GateProxy is ERC1967Proxy {
    error MissingInitializer();

    constructor(address implementation, bytes memory data)
        ERC1967Proxy(implementation, data)
    {
        if (data.length == 0) revert MissingInitializer();
    }
}
