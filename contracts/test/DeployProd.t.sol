// SPDX-License-Identifier: MIT
pragma solidity 0.8.24;

import {Test} from "forge-std/Test.sol";
import {Gate} from "../src/Gate.sol";
import {DeployProd} from "../script/DeployProd.s.sol";

contract DeployProdTest is Test {
    DeployProd dep;

    address gv1 = address(0xA11CE);
    address gv2 = address(0xB0B);
    address gv3 = address(0xC0FFEE);
    address guardian = address(0x6A2D);
    address multisig = address(0x5AFE);

    function setUp() public {
        dep = new DeployProd();
    }

    function _validators() internal view returns (address[] memory v) {
        v = new address[](3);
        v[0] = gv1;
        v[1] = gv2;
        v[2] = gv3;
    }

    function _params() internal view returns (DeployProd.Params memory) {
        return DeployProd.Params({
            expectedChainId: block.chainid,
            validators: _validators(),
            threshold: 2, // strict majority of 3
            guardian: guardian,
            owner: multisig
        });
    }

    function test_Deploy_HappyPath_SetsAllInvariants() public {
        Gate gate = dep._deploy(_params());

        assertEq(gate.validatorCount(), 3);
        assertEq(gate.threshold(), 2);
        assertEq(gate.guardian(), guardian);
        assertEq(gate.pendingOwner(), multisig, "ownership handover must be pending to the multisig");
        // owner is still the deployer until the multisig accepts (two-step).
        assertEq(gate.owner(), address(dep));
        assertTrue(gate.isValidator(gv1) && gate.isValidator(gv2) && gate.isValidator(gv3));

        // finishing the handover works and revokes deployer control.
        vm.prank(multisig);
        gate.acceptOwnership();
        assertEq(gate.owner(), multisig);
    }

    function test_Deploy_RejectsWrongChain() public {
        DeployProd.Params memory p = _params();
        p.expectedChainId = block.chainid + 1;
        vm.expectRevert(
            abi.encodeWithSelector(DeployProd.WrongChain.selector, block.chainid, block.chainid + 1)
        );
        dep._deploy(p);
    }

    function test_Deploy_RejectsThresholdOne() public {
        DeployProd.Params memory p = _params();
        p.threshold = 1; // the demo default — must be rejected in prod
        vm.expectRevert(abi.encodeWithSelector(DeployProd.WeakThreshold.selector, 1, 3));
        dep._deploy(p);
    }

    function test_Deploy_RejectsSubMajorityThreshold() public {
        // 2-of-5 is not a majority.
        address[] memory v = new address[](5);
        v[0] = gv1;
        v[1] = gv2;
        v[2] = gv3;
        v[3] = address(0xD00D);
        v[4] = address(0xE11E);
        DeployProd.Params memory p = _params();
        p.validators = v;
        p.threshold = 2;
        vm.expectRevert(abi.encodeWithSelector(DeployProd.WeakThreshold.selector, 2, 5));
        dep._deploy(p);
    }

    function test_Deploy_RejectsTooFewValidators() public {
        address[] memory v = new address[](2);
        v[0] = gv1;
        v[1] = gv2;
        DeployProd.Params memory p = _params();
        p.validators = v;
        p.threshold = 2;
        vm.expectRevert(abi.encodeWithSelector(DeployProd.TooFewValidators.selector, 2));
        dep._deploy(p);
    }

    function test_Deploy_RejectsZeroGuardianOrOwner() public {
        DeployProd.Params memory p = _params();
        p.guardian = address(0);
        vm.expectRevert(DeployProd.ZeroConfigAddress.selector);
        dep._deploy(p);
    }

    /// L-1: Gate's constructor dedupes as it registers, so `[A, B, B]` used to
    /// pass every preflight rule (all computed against the SUPPLIED array of 3)
    /// AND every post-deploy assertion, yet ship a 2-of-2 gate — one key short of
    /// the quorum the operator signed off on.
    function test_Deploy_RejectsDuplicateValidators() public {
        DeployProd.Params memory p = _params();
        p.validators[2] = p.validators[1]; // [A, B, B] — still length 3

        vm.expectRevert(
            abi.encodeWithSelector(DeployProd.DuplicateValidator.selector, p.validators[1])
        );
        dep._deploy(p);
    }

    /// A zero entry would be rejected by Gate's own constructor, but failing in
    /// preflight gives the operator the specific reason before any gas is spent.
    function test_Deploy_RejectsZeroValidator() public {
        DeployProd.Params memory p = _params();
        p.validators[1] = address(0);

        vm.expectRevert(DeployProd.ZeroValidatorAddress.selector);
        dep._deploy(p);
    }

    /// The post-deploy assertion that pins the invariant the duplicate check
    /// establishes: what was registered equals what was supplied.
    function test_Deploy_ValidatorCountEqualsSuppliedLength() public {
        Gate gate = dep._deploy(_params());
        assertEq(gate.validatorCount(), _validators().length, "dedupe must not shrink the set");
        assertTrue(gate.threshold() * 2 > gate.validatorCount(), "threshold is a strict majority");
    }

    function test_Deploy_RejectsGuardianEqualsOwner() public {
        DeployProd.Params memory p = _params();
        p.guardian = multisig;
        vm.expectRevert(DeployProd.GuardianEqualsOwner.selector);
        dep._deploy(p);
    }
}
