// SPDX-License-Identifier: MIT
pragma solidity 0.8.24;

import {GateProxy} from "../../src/GateProxy.sol";
import {Gate} from "../../src/Gate.sol";
import {GateDeployer} from "../../src/GateDeployer.sol";

/// @dev The deployment domain every unit test shares. A fixed non-zero value:
///      tests that care about domain SEPARATION (see Upgrade.t.sol) deploy a
///      second mesh with a different one and assert the ids diverge.
bytes32 constant TEST_BRIDGE_DOMAIN = keccak256("selendra.bridge.test.v1");

/// @dev Stand up a proxied Gate with the shared test domain.
///
///      Deliberately a thin wrapper over {GateDeployer.deploy} rather than a
///      test-only shortcut that skips the proxy: the tests must exercise the
///      same delegatecall path production runs, or they would not catch an
///      initializer that reverts only behind a proxy.
function deployTestGate(address[] memory validators, uint256 threshold) returns (Gate) {
    return GateDeployer.deploy(validators, threshold, TEST_BRIDGE_DOMAIN);
}

/// @dev Same thing, but against an implementation the caller already deployed.
///
///      Exists for `vm.expectRevert`. {deployTestGate} performs TWO creates —
///      the implementation, then the proxy — and `expectRevert` binds to the
///      NEXT call, which is the implementation create. That one always succeeds,
///      so the assertion fails with "next call did not revert" even when
///      `initialize` reverts exactly as intended. Hoisting `new Gate()` out
///      leaves the proxy create as the next call, which is the one under test.
function initTestGate(Gate implementation, address[] memory validators, uint256 threshold)
    returns (Gate)
{
    GateProxy proxy = new GateProxy(
        address(implementation),
        abi.encodeCall(Gate.initialize, (validators, threshold, TEST_BRIDGE_DOMAIN))
    );
    return Gate(address(proxy));
}
