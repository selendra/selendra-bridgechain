// SPDX-License-Identifier: MIT
pragma solidity 0.8.24;

import {Script, console2} from "forge-std/Script.sol";
import {Gate} from "../src/Gate.sol";

/// @notice Production Gate deployment with enforced safety parameters.
///
/// Unlike the demo scripts (which deploy single-validator, threshold-1 gates and
/// unrestricted-mint test tokens for local bring-up), this script:
///   * guards the target chain id (a fat-fingered RPC can't deploy to the wrong
///     network);
///   * requires a real multi-validator set and a STRICT-MAJORITY threshold
///     (never threshold-1, which a single compromised signer could abuse);
///   * appoints a guardian (low-trust pause button, separable from the owner);
///   * hands ownership to a multisig via the two-step transfer (the multisig must
///     call acceptOwnership to finish);
///   * deploys NO mintable tokens — real assets are registered later, per asset,
///     by governance via setLocalToken;
///   * asserts every post-deploy invariant and reverts the whole deployment if
///     any is off.
///
/// The deployment logic lives in `_deploy(Params)` so it can be unit-tested
/// (see test/DeployProd.t.sol) without env plumbing or a live RPC.
///
/// Env in (for `forge script`):
///   EXPECTED_CHAIN_ID (uint)  — the chain this MUST run on
///   VALIDATORS (address[],",") — comma-separated validator addresses (>= 3)
///   THRESHOLD (uint)          — strict-majority signature threshold (>= 2)
///   GUARDIAN (address)        — the pause guardian (nonzero, != owner)
///   OWNER (address)           — the multisig to receive ownership (nonzero)
contract DeployProd is Script {
    struct Params {
        uint256 expectedChainId;
        address[] validators;
        uint256 threshold;
        address guardian;
        address owner;
    }

    error WrongChain(uint256 got, uint256 want);
    error TooFewValidators(uint256 count);
    error WeakThreshold(uint256 threshold, uint256 validatorCount);
    error ZeroConfigAddress();
    error GuardianEqualsOwner();

    function run() external {
        Params memory p = Params({
            expectedChainId: vm.envUint("EXPECTED_CHAIN_ID"),
            validators: vm.envAddress("VALIDATORS", ","),
            threshold: vm.envUint("THRESHOLD"),
            guardian: vm.envAddress("GUARDIAN"),
            owner: vm.envAddress("OWNER")
        });

        vm.startBroadcast();
        Gate gate = _deploy(p);
        vm.stopBroadcast();

        console2.log("Gate deployed:", address(gate));
        console2.log("  validatorCount:", gate.validatorCount());
        console2.log("  threshold:", gate.threshold());
        console2.log("  guardian:", gate.guardian());
        console2.log("  pendingOwner (accept from multisig):", gate.pendingOwner());
    }

    /// @dev Deploy + configure + assert. Public so tests can exercise it; the
    ///      caller (broadcaster or test) becomes the transient owner that appoints
    ///      the guardian and starts the ownership handover.
    function _deploy(Params memory p) public returns (Gate gate) {
        // --- production policy pre-flight (revert before spending gas on deploy) ---
        if (block.chainid != p.expectedChainId) revert WrongChain(block.chainid, p.expectedChainId);
        if (p.validators.length < 3) revert TooFewValidators(p.validators.length);
        if (p.guardian == address(0) || p.owner == address(0)) revert ZeroConfigAddress();
        if (p.guardian == p.owner) revert GuardianEqualsOwner();
        // Strict majority: threshold > validators/2 AND at least 2. This forbids
        // the demo threshold-1 and any sub-majority quorum. (Gate's own constructor
        // only rejects 0 and > count.)
        if (p.threshold < 2 || p.threshold * 2 <= p.validators.length || p.threshold > p.validators.length) {
            revert WeakThreshold(p.threshold, p.validators.length);
        }

        gate = new Gate(p.validators, p.threshold);
        gate.setGuardian(p.guardian);
        gate.transferOwnership(p.owner); // two-step: p.owner must acceptOwnership()

        // --- post-deploy assertions (revert the deployment if any fails) ---
        require(gate.threshold() == p.threshold, "post: threshold mismatch");
        require(gate.guardian() == p.guardian, "post: guardian mismatch");
        require(gate.pendingOwner() == p.owner, "post: pendingOwner mismatch");
        require(gate.validatorCount() >= p.threshold, "post: validatorCount < threshold");
        for (uint256 i = 0; i < p.validators.length; i++) {
            require(gate.isValidator(p.validators[i]), "post: validator not registered");
        }
    }
}
