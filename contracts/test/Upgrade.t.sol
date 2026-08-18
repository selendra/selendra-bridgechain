// SPDX-License-Identifier: MIT
pragma solidity 0.8.24;

import {Test} from "forge-std/Test.sol";
import {Gate} from "../src/Gate.sol";
import {GateDeployer} from "../src/GateDeployer.sol";
import {TestToken} from "../src/TestToken.sol";
import {BridgeHash} from "../src/BridgeHash.sol";
import {ERC1967Proxy} from "@openzeppelin/contracts/proxy/ERC1967/ERC1967Proxy.sol";
import {MessageHashUtils} from "@openzeppelin/contracts/utils/cryptography/MessageHashUtils.sol";

/// @dev A trivial next version, only so an upgrade has somewhere to go.
contract GateV2 is Gate {
    function version() external pure returns (string memory) {
        return "v2";
    }
}

/// @notice Covers the two properties added after the live redeploy of 2026-08-14:
///
///         1. **Cross-deployment replay is impossible** (finding H-3). A
///            submissionId now commits to `bridgeDomain`, so a quorum signature
///            produced for one deployment generation cannot authorise a payout
///            from the next one.
///
///         2. **The gate is upgradeable in place, but only slowly.** Upgrading
///            rather than redeploying is what keeps one address, one storage and
///            one `nonceTo` — which is what removes the *occasion* for a replay
///            in the first place. The timelock is what stops the upgrade power
///            from simply becoming a faster way to take the funds.
contract UpgradeTest is Test {
    bytes32 constant DOMAIN_A = keccak256("mesh.generation.A");
    bytes32 constant DOMAIN_B = keccak256("mesh.generation.B");

    uint256 constant CHAIN_SRC = 1337;
    uint256 constant CHAIN_DST = 1338;
    uint256 constant AMOUNT = 100 ether;

    uint256 v1pk = 0xA11CE;
    address v1;
    address[] validators;

    address user = address(0x5E4DE2);
    address receiverAddr = address(0xCAFE);
    bytes receiver;
    bytes EMPTY = "";

    function setUp() public {
        v1 = vm.addr(v1pk);
        validators = new address[](1);
        validators[0] = v1;
        receiver = abi.encodePacked(receiverAddr);
    }

    function _sign(uint256 pk, bytes32 message) internal pure returns (bytes[] memory sigs) {
        (uint8 v, bytes32 r, bytes32 s) =
            vm.sign(pk, MessageHashUtils.toEthSignedMessageHash(message));
        sigs = new bytes[](1);
        sigs[0] = abi.encodePacked(r, s, v);
    }

    /// @dev Stand up a destination gate that is fully able to pay: registered
    ///      corridor plus real liquidity. Used to build both the legitimate
    ///      destination and the "redeployed" one the replay is aimed at.
    function _destination(bytes32 domain, bytes32 debridgeId) internal returns (Gate g) {
        g = GateDeployer.deploy(validators, 1, domain);
        TestToken dstToken = new TestToken("Test", "TST");
        dstToken.mint(address(g), 1_000 ether);
        g.setLocalToken(debridgeId, address(dstToken));
    }

    // -----------------------------------------------------------------
    // H-3 — cross-deployment attestation replay
    // -----------------------------------------------------------------

    /// The exact shape of the live incident: a fresh destination gate, same
    /// validators, same asset, `nonceTo` back at 0 — and a stale attestation
    /// aimed at it. Under a NEW domain the signature no longer authorises
    /// anything, so the fresh gate's liquidity is untouchable.
    function test_ReplayAcrossDeployments_IsRejectedByTheDomain() public {
        vm.chainId(CHAIN_SRC);
        Gate srcGate = GateDeployer.deploy(validators, 1, DOMAIN_A);
        TestToken token = new TestToken("Test", "TST");
        token.mint(user, 1_000 ether);

        vm.startPrank(user);
        token.approve(address(srcGate), AMOUNT);
        bytes32 submissionId = srcGate.send(address(token), AMOUNT, CHAIN_DST, receiver, EMPTY);
        vm.stopPrank();

        bytes32 debridgeId = BridgeHash.getDebridgeId(CHAIN_SRC, address(token));
        bytes[] memory sigs = _sign(v1pk, submissionId);

        // --- generation A: the attestation is genuinely valid here ---
        vm.chainId(CHAIN_DST);
        Gate genA = _destination(DOMAIN_A, debridgeId);
        genA.claim(debridgeId, AMOUNT, CHAIN_SRC, 0, receiver, EMPTY, EMPTY, sigs);
        assertEq(
            TestToken(genA.tokenOf(debridgeId)).balanceOf(receiverAddr),
            AMOUNT,
            "control: the signature really does authorise a payout in its own generation"
        );

        // --- generation B: same everything, new domain ---
        Gate genB = _destination(DOMAIN_B, debridgeId);
        assertEq(genB.nonceTo(CHAIN_SRC), 0, "a fresh gate restarts its nonces: the replay's opening");

        // The gate recomputes the id under DOMAIN_B, so the signature recovers a
        // non-validator and the quorum is never met.
        vm.expectRevert(abi.encodeWithSelector(Gate.NotEnoughSignatures.selector, 0, 1));
        genB.claim(debridgeId, AMOUNT, CHAIN_SRC, 0, receiver, EMPTY, EMPTY, sigs);

        assertEq(
            TestToken(genB.tokenOf(debridgeId)).balanceOf(receiverAddr),
            0,
            "the redeployed gate must not pay out against the previous generation's deposit"
        );
        assertEq(
            TestToken(genB.tokenOf(debridgeId)).balanceOf(address(genB)),
            1_000 ether,
            "the redeployed gate keeps its full liquidity"
        );
    }

    /// The other half of the lesson, pinned so nobody "simplifies" the domain
    /// into a constant: reusing a domain across generations is STILL replayable.
    /// The protection comes from rotating it, not from its mere presence.
    function test_ReusingTheDomainOnRedeploy_IsStillReplayable() public {
        vm.chainId(CHAIN_SRC);
        Gate srcGate = GateDeployer.deploy(validators, 1, DOMAIN_A);
        TestToken token = new TestToken("Test", "TST");
        token.mint(user, 1_000 ether);

        vm.startPrank(user);
        token.approve(address(srcGate), AMOUNT);
        bytes32 submissionId = srcGate.send(address(token), AMOUNT, CHAIN_DST, receiver, EMPTY);
        vm.stopPrank();

        bytes32 debridgeId = BridgeHash.getDebridgeId(CHAIN_SRC, address(token));
        bytes[] memory sigs = _sign(v1pk, submissionId);

        vm.chainId(CHAIN_DST);
        Gate redeployedSameDomain = _destination(DOMAIN_A, debridgeId);
        redeployedSameDomain.claim(debridgeId, AMOUNT, CHAIN_SRC, 0, receiver, EMPTY, EMPTY, sigs);

        assertEq(
            TestToken(redeployedSameDomain.tokenOf(debridgeId)).balanceOf(receiverAddr),
            AMOUNT,
            "documented hazard: a redeploy that keeps the domain replays exactly as before"
        );
    }

    function test_DomainIsRecordedAndImmutablyPartOfTheId() public {
        vm.chainId(CHAIN_SRC);
        Gate a = GateDeployer.deploy(validators, 1, DOMAIN_A);
        Gate b = GateDeployer.deploy(validators, 1, DOMAIN_B);
        assertEq(a.bridgeDomain(), DOMAIN_A);
        assertEq(b.bridgeDomain(), DOMAIN_B);

        bytes32 debridgeId = BridgeHash.getDebridgeId(CHAIN_SRC, address(0x1234));
        assertTrue(
            a.computeSubmissionId(debridgeId, AMOUNT, CHAIN_SRC, CHAIN_DST, 0, receiver, EMPTY, EMPTY)
                != b.computeSubmissionId(debridgeId, AMOUNT, CHAIN_SRC, CHAIN_DST, 0, receiver, EMPTY, EMPTY),
            "two generations must never agree on a submissionId"
        );
    }

    function test_Initialize_RejectsZeroDomain() public {
        Gate impl = new Gate();
        vm.expectRevert(Gate.ZeroBridgeDomain.selector);
        new ERC1967Proxy(
            address(impl), abi.encodeCall(Gate.initialize, (validators, 1, bytes32(0)))
        );
    }

    // -----------------------------------------------------------------
    // Upgradeability
    // -----------------------------------------------------------------

    function test_Upgrade_RequiresASchedule() public {
        Gate gate = GateDeployer.deploy(validators, 1, DOMAIN_A);
        address v2 = address(new GateV2());
        vm.expectRevert(abi.encodeWithSelector(Gate.UpgradeNotScheduled.selector, v2));
        gate.upgradeToAndCall(v2, "");
    }

    function test_Upgrade_RequiresTheDelayToElapse() public {
        Gate gate = GateDeployer.deploy(validators, 1, DOMAIN_A);
        address v2 = address(new GateV2());
        gate.scheduleUpgrade(v2);
        uint256 readyAt = gate.upgradeReadyAt(v2);

        vm.warp(readyAt - 1);
        vm.expectRevert(abi.encodeWithSelector(Gate.UpgradeNotReady.selector, v2, readyAt));
        gate.upgradeToAndCall(v2, "");
    }

    function test_Upgrade_SucceedsAfterTheDelay_AndKeepsAllState() public {
        vm.chainId(CHAIN_SRC);
        Gate gate = GateDeployer.deploy(validators, 1, DOMAIN_A);
        TestToken token = new TestToken("Test", "TST");
        token.mint(user, 1_000 ether);

        vm.startPrank(user);
        token.approve(address(gate), AMOUNT);
        bytes32 submissionId = gate.send(address(token), AMOUNT, CHAIN_DST, receiver, EMPTY);
        vm.stopPrank();

        uint256 lockedBefore = token.balanceOf(address(gate));
        uint256 nonceBefore = gate.nonceTo(CHAIN_DST);

        address v2 = address(new GateV2());
        gate.scheduleUpgrade(v2);
        vm.warp(block.timestamp + gate.UPGRADE_DELAY());
        gate.upgradeToAndCall(v2, "");

        assertEq(GateV2(address(gate)).version(), "v2", "new logic is live");
        // Everything that makes the gate trustworthy has to survive the swap —
        // the locked funds above all, but equally the nonce and the origin proof,
        // because losing either is what a replay needs.
        assertEq(token.balanceOf(address(gate)), lockedBefore, "locked funds preserved");
        assertEq(gate.nonceTo(CHAIN_DST), nonceBefore, "nonce preserved");
        assertEq(gate.sentBy(submissionId), user, "origin proof preserved");
        assertEq(gate.bridgeDomain(), DOMAIN_A, "domain preserved");
        assertEq(gate.threshold(), 1, "validator config preserved");
        assertTrue(gate.isValidator(v1), "validator set preserved");
    }

    function test_Upgrade_ScheduleIsConsumed_SoOneApprovalInstallsOnce() public {
        Gate gate = GateDeployer.deploy(validators, 1, DOMAIN_A);
        address v2 = address(new GateV2());
        gate.scheduleUpgrade(v2);
        vm.warp(block.timestamp + gate.UPGRADE_DELAY());
        gate.upgradeToAndCall(v2, "");

        assertEq(gate.upgradeReadyAt(v2), 0, "the approval must be burned on use");

        // Rolling back to v2 again (after some other upgrade) must re-queue.
        vm.expectRevert(abi.encodeWithSelector(Gate.UpgradeNotScheduled.selector, v2));
        gate.upgradeToAndCall(v2, "");
    }

    function test_Upgrade_OnlyOwnerCanSchedule() public {
        Gate gate = GateDeployer.deploy(validators, 1, DOMAIN_A);
        address v2 = address(new GateV2());
        vm.prank(address(0xBADA55));
        vm.expectRevert(Gate.NotOwner.selector);
        gate.scheduleUpgrade(v2);
    }

    function test_Upgrade_OnlyOwnerCanExecute() public {
        Gate gate = GateDeployer.deploy(validators, 1, DOMAIN_A);
        address v2 = address(new GateV2());
        gate.scheduleUpgrade(v2);
        vm.warp(block.timestamp + gate.UPGRADE_DELAY());

        vm.prank(address(0xBADA55));
        vm.expectRevert(Gate.NotOwner.selector);
        gate.upgradeToAndCall(v2, "");
    }

    function test_Upgrade_GuardianCanCancelAPendingUpgrade() public {
        Gate gate = GateDeployer.deploy(validators, 1, DOMAIN_A);
        address guardian = address(0x6A11);
        gate.setGuardian(guardian);

        address v2 = address(new GateV2());
        gate.scheduleUpgrade(v2);

        vm.prank(guardian);
        gate.cancelScheduledUpgrade(v2);
        assertEq(gate.upgradeReadyAt(v2), 0);

        vm.warp(block.timestamp + gate.UPGRADE_DELAY());
        vm.expectRevert(abi.encodeWithSelector(Gate.UpgradeNotScheduled.selector, v2));
        gate.upgradeToAndCall(v2, "");
    }

    function test_Upgrade_ReschedulingRestartsTheDelay() public {
        Gate gate = GateDeployer.deploy(validators, 1, DOMAIN_A);
        address v2 = address(new GateV2());

        gate.scheduleUpgrade(v2);
        vm.warp(block.timestamp + gate.UPGRADE_DELAY());

        // Re-scheduling must NOT keep the already-matured deadline, or an owner
        // could bank a permanent instant-upgrade right against this address.
        gate.scheduleUpgrade(v2);
        uint256 readyAt = gate.upgradeReadyAt(v2);
        assertEq(readyAt, block.timestamp + gate.UPGRADE_DELAY(), "delay must restart");

        vm.expectRevert(abi.encodeWithSelector(Gate.UpgradeNotReady.selector, v2, readyAt));
        gate.upgradeToAndCall(v2, "");
    }

    /// The implementation must be permanently uninitializable, or anyone can own
    /// it and drive its own `upgradeToAndCall`.
    function test_Implementation_CannotBeInitialized() public {
        Gate impl = new Gate();
        vm.expectRevert(); // OZ InvalidInitialization
        impl.initialize(validators, 1, DOMAIN_A);
        assertEq(impl.owner(), address(0), "implementation must never have an owner");
    }
}
