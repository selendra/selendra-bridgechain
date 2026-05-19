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
}
