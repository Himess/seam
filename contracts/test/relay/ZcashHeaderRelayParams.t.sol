// SPDX-License-Identifier: MIT
pragma solidity 0.8.28;

import {RelayFixtures} from "./RelayFixtures.sol";
import {ZcashHeaderRelay} from "../../src/relay/ZcashHeaderRelay.sol";
import {IZcashHeaderRelay, ZCASH_MAINNET, ZCASH_TESTNET} from "../../src/relay/IZcashHeaderRelay.sol";
import {ZcashEquihash} from "../../src/relay/ZcashEquihash.sol";
import {ZcashPow} from "../../src/relay/ZcashPow.sol";
import {DeployRelay} from "../../script/DeployRelay.s.sol";

/// @notice The relay's network parameters: constructor validation, that each parameter is honoured, and the
///         deployment configuration files (config/relay-<network>.json) read by script/DeployRelay.s.sol.
contract ZcashHeaderRelayParamsTest is RelayFixtures {
    // --- network ids and parameters -------------------------------------------------------------------------

    function test_networkIds() public pure {
        assertEq(ZCASH_TESTNET, keccak256("zcash-testnet"));
        assertEq(ZCASH_MAINNET, keccak256("zcash-mainnet"));
    }

    function test_testnetRelayReportsItsParameters() public {
        ZcashHeaderRelay relay = _deploy(_loadReal(RECENT));
        assertEq(relay.networkId(), ZCASH_TESTNET);
        assertEq(abi.encode(relay.params()), abi.encode(_testnetParams()));
        assertEq(relay.CONTEXT_LENGTH(), 113);
        assertEq(relay.NEVER(), type(uint32).max);
    }

    /// @notice The PoWLimit words are the compact forms of 2^251 - 1 and 2^243 - 1 (protocol specification § 5.3).
    function test_powLimitsAreTheRoundedSpecificationValues() public pure {
        assertEq(ZcashPow.encodeBits((uint256(1) << 251) - 1), _testnetParams().powLimitBits);
        assertEq(ZcashPow.encodeBits((uint256(1) << 243) - 1), _mainnetParams().powLimitBits);
        (uint256 t,) = ZcashPow.decodeBits(0x2007ffff);
        assertEq(t, _testnetParams().powLimit);
        (t,) = ZcashPow.decodeBits(0x1f07ffff);
        assertEq(t, _mainnetParams().powLimit);
        assertEq(ZcashPow.work(_mainnetParams().powLimit), 8192); // 2^256 / 2^243
    }

    // --- constructor validation ---------------------------------------------------------------------------------

    function test_constructor_rejectsBadParameters() public {
        RealFixture memory fx = _loadReal(RECENT);

        ZcashHeaderRelay.RelayParams memory p = _testnetParams();
        p.networkId = bytes32(0);
        _expectConstructorRevert(fx, p, IZcashHeaderRelay.InvalidNetworkId.selector);

        p = _testnetParams();
        p.powLimit = (uint256(1) << 251) - 1; // not rounded through the compact form
        _expectConstructorRevert(fx, p, IZcashHeaderRelay.InvalidPowLimit.selector);
        p = _testnetParams();
        p.powLimitBits = 0x1f07ffff; // the other network's bits
        _expectConstructorRevert(fx, p, IZcashHeaderRelay.InvalidPowLimit.selector);
        p.powLimitBits = 0x20812345; // negative
        _expectConstructorRevert(fx, p, IZcashHeaderRelay.InvalidPowLimit.selector);
        p.powLimitBits = 0;
        p.powLimit = 0;
        _expectConstructorRevert(fx, p, IZcashHeaderRelay.InvalidPowLimit.selector);
        p.powLimitBits = 0x20000007; // decodes to 7 << 232, whose canonical form is 0x1e070000
        p.powLimit = uint256(7) << 232;
        _expectConstructorRevert(fx, p, IZcashHeaderRelay.InvalidPowLimit.selector);
        p.powLimitBits = 0x21008000; // 2^255, canonical: ThresholdBits (up to 1.32 x the mean) could overflow
        p.powLimit = uint256(1) << 255;
        _expectConstructorRevert(fx, p, IZcashHeaderRelay.InvalidPowLimit.selector);
        p.powLimitBits = 0x207fffff; // the largest canonical value below 2^255 is accepted
        p.powLimit = uint256(0x7fffff) << 232;
        this.deployRelay(p, fx.checkpointHash, fx.checkpointHeight, fx.contextTimes, fx.contextBits);

        // Zebra asserts that minimum-difficulty blocks never occur on Mainnet.
        p = _mainnetParams();
        p.minDifficultyBlocks = true;
        p.minDifficultyStartHeight = 299_188;
        _expectConstructorRevert(fx, p, IZcashHeaderRelay.InvalidMinDifficultyRule.selector);
        p = _testnetParams();
        p.minDifficultyBlocks = false; // the start height must then be 0
        _expectConstructorRevert(fx, p, IZcashHeaderRelay.InvalidMinDifficultyRule.selector);

        p = _testnetParams();
        p.preNu7TargetSpacing = 0;
        _expectConstructorRevert(fx, p, IZcashHeaderRelay.InvalidDifficultyAdjustment.selector);
        p = _testnetParams();
        p.postNu7TargetSpacing = 0;
        _expectConstructorRevert(fx, p, IZcashHeaderRelay.InvalidDifficultyAdjustment.selector);
        p = _testnetParams();
        p.preNu7AveragingWindow = 0;
        _expectConstructorRevert(fx, p, IZcashHeaderRelay.InvalidDifficultyAdjustment.selector);
        p = _testnetParams();
        p.postNu7AveragingWindow = 103; // the 113 context blocks cover windows up to 102
        _expectConstructorRevert(fx, p, IZcashHeaderRelay.InvalidDifficultyAdjustment.selector);

        p = _testnetParams();
        p.minCheckpointHeight = 111; // the context would start below the genesis block
        _expectConstructorRevert(fx, p, IZcashHeaderRelay.InvalidMinCheckpointHeight.selector);
        assertEq(_constructorRevertsChecked, 14, "every case ran");
    }

    function test_constructor_checkpointHeightBounds() public {
        RealFixture memory fx = _loadReal(MAINNET_RECENT);
        ZcashHeaderRelay.RelayParams memory p = _mainnetParams();
        vm.expectRevert(IZcashHeaderRelay.InvalidCheckpoint.selector);
        this.deployRelay(p, fx.checkpointHash, 653_599, fx.contextTimes, fx.contextBits);
        this.deployRelay(p, fx.checkpointHash, 653_600, fx.contextTimes, fx.contextBits); // Blossom
        p.minCheckpointHeight = 112;
        this.deployRelay(p, fx.checkpointHash, 112, fx.contextTimes, fx.contextBits); // context from height 0
        vm.expectRevert(IZcashHeaderRelay.InvalidCheckpoint.selector);
        this.deployRelay(p, fx.checkpointHash, 2 ** 31, fx.contextTimes, fx.contextBits);
        this.deployRelay(p, fx.checkpointHash, 2 ** 31 - 1, fx.contextTimes, fx.contextBits);
    }

    function _expectConstructorRevert(RealFixture memory fx, ZcashHeaderRelay.RelayParams memory p, bytes4 selector)
        internal
    {
        vm.expectRevert(selector);
        this.deployRelay(p, fx.checkpointHash, fx.checkpointHeight, fx.contextTimes, fx.contextBits);
        ++_constructorRevertsChecked;
    }

    uint256 internal _constructorRevertsChecked;

    // --- every parameter is applied -----------------------------------------------------------------------------

    /// @notice With the 90-minute rule switched off (`NEVER`), a header 90 min + 1 s after the median-time-past
    ///         gets past the time rules and fails only because the changed time breaks its Equihash solution.
    function test_maxTimeStartHeightIsApplied() public {
        RealFixture memory fx = _loadReal(MAINNET_RECENT);
        uint256 mtp = _medianOfLast11(fx.contextTimes);
        bytes memory h = _copy(fx.headers[0]);
        _setLe32(h, 100, mtp + 90 minutes + 1);
        vm.warp(mtp + 90 minutes);

        ZcashHeaderRelay.RelayParams memory p = _mainnetParams();
        p.maxTimeStartHeight = uint32(fx.checkpointHeight + 1); // from this header on: refused
        ZcashHeaderRelay relay = _deploy(fx, p);
        vm.expectRevert(
            abi.encodeWithSelector(IZcashHeaderRelay.TimeTooLate.selector, mtp + 90 minutes + 1, mtp + 90 minutes)
        );
        relay.submitHeader(h);

        p.maxTimeStartHeight = uint32(fx.checkpointHeight + 2); // from the next height on: not yet
        relay = _deploy(fx, p);
        vm.expectRevert(
            abi.encodeWithSelector(IZcashHeaderRelay.InvalidEquihashSolution.selector, ZcashEquihash.Result.Collision)
        );
        relay.submitHeader(h);

        p.maxTimeStartHeight = type(uint32).max; // never
        relay = _deploy(fx, p);
        vm.expectRevert(
            abi.encodeWithSelector(IZcashHeaderRelay.InvalidEquihashSolution.selector, ZcashEquihash.Result.Collision)
        );
        relay.submitHeader(h);
    }

    /// @notice The minimum-difficulty rule applies from `minDifficultyStartHeight` only: below it, the genuine Testnet
    ///         minimum-difficulty block A1 needs the retarget value like any other block.
    function test_minDifficultyStartHeightIsApplied() public {
        RealFixture memory fx = _loadReal(RECENT);
        (bytes memory a1,) = _fork(_synthetic(), "A1");
        ZcashHeaderRelay.RelayParams memory p = _testnetParams();
        p.minDifficultyStartHeight = uint32(fx.heights[3] + 1); // A1 is at fx.heights[3]
        ZcashHeaderRelay relay = _deploy(fx, p);
        _submitFirst(relay, fx, 3);
        vm.warp(_time(a1) + 60);
        vm.expectRevert(
            abi.encodeWithSelector(IZcashHeaderRelay.BadDifficultyBits.selector, 0x2007ffff, _bits(fx.headers[3]))
        );
        relay.submitHeader(a1);

        p.minDifficultyStartHeight = uint32(fx.heights[3]); // from A1's height on
        relay = _deploy(fx, p);
        _submitFirst(relay, fx, 3);
        vm.warp(_time(a1) + 60);
        relay.submitHeader(a1);
    }

    /// @notice The NU7 activation height decides the adjustment: with Testnet's NU7 one block later, the real
    ///         activation block 4,465,026 fails, since the pre-NU7 rules (75 s, 17 blocks) give 0x1e7c37e1 (value
    ///         from the Python model, tools/zcash_header_check.py) where the block carries the NU7 value.
    function test_nu7ActivationHeightIsApplied() public {
        RealFixture memory fx = _loadReal(NU7_ACTIVATION);
        assertEq(fx.heights[11], 4_465_026);
        ZcashHeaderRelay.RelayParams memory p = _testnetParams();
        p.nu7ActivationHeight = 4_465_027;
        ZcashHeaderRelay relay = _deploy(fx, p);
        _submitFirst(relay, fx, 11); // the 11 pre-NU7 blocks pass either way
        _warpTo(fx.headers[11]);
        vm.expectRevert(abi.encodeWithSelector(IZcashHeaderRelay.BadDifficultyBits.selector, 0x1f0162f6, 0x1e7c37e1));
        relay.submitHeader(fx.headers[11]);
    }

    // --- deployment configuration (script/DeployRelay.s.sol) ----------------------------------------------------

    /// @notice The configuration files hold exactly the parameters the tests use (a dry run of their parsing).
    function test_config_filesMatchTheTestedParameters() public {
        DeployRelay script = new DeployRelay();
        (ZcashHeaderRelay.RelayParams memory p, string memory name, string memory display) =
            script.loadParams("config/relay-testnet.json");
        assertEq(abi.encode(p), abi.encode(_testnetParams()));
        assertEq(name, "zcash-testnet");
        assertEq(display, "Zcash Testnet");
        (p, name, display) = script.loadParams("config/relay-mainnet.json");
        assertEq(abi.encode(p), abi.encode(_mainnetParams()));
        assertEq(name, "zcash-mainnet");
        assertEq(display, "Zcash Mainnet");
    }

    /// @notice A relay deployed from a configuration and a checkpoint file accepts that network's next header.
    function test_config_deploysWorkingRelays() public {
        DeployRelay script = new DeployRelay();
        _deployFromConfig(script, "config/relay-testnet.json", RECENT);
        _deployFromConfig(script, "config/relay-mainnet.json", MAINNET_RECENT);
        // The Base Sepolia checkpoint file of the deployed Testnet relay still loads, and it is that relay's checkpoint
        // (its record, deployments/base-sepolia.relay.json, names it).
        (,, string memory display) = script.loadParams("config/relay-testnet.json");
        DeployRelay.Checkpoint memory cp =
            script.loadCheckpoint("deployments/base-sepolia.relay-checkpoint.json", display);
        string memory record = vm.readFile("deployments/base-sepolia.relay.json");
        assertEq(cp.height, vm.parseJsonUint(record, ".checkpointHeight"));
        assertEq(cp.hash, vm.parseJsonBytes32(record, ".checkpointHash"));
        assertEq(cp.times.length, 113);
    }

    /// @notice A checkpoint file that names its network must name the configuration's.
    function test_config_refusesACheckpointOfAnotherNetwork() public {
        DeployRelay script = new DeployRelay();
        vm.expectRevert(bytes("DeployRelay: the checkpoint is on Zcash Mainnet, the parameters are for Zcash Testnet"));
        script.loadCheckpoint(MAINNET_RECENT, "Zcash Testnet");
        vm.expectRevert(bytes("DeployRelay: the checkpoint is on Zcash Testnet, the parameters are for Zcash Mainnet"));
        script.loadCheckpoint(RECENT, "Zcash Mainnet");
    }

    /// @notice Every deployment names its record, so that none overwrites another's (base-sepolia is the record of
    ///         Zecured's live run).
    function test_config_deploymentName() public {
        DeployRelay script = new DeployRelay();
        assertEq(script.deploymentName(ZCASH_TESTNET, "anvil"), "anvil");
        assertEq(script.deploymentName(ZCASH_MAINNET, "base-sepolia-mainnet-shadow"), "base-sepolia-mainnet-shadow");
        vm.expectRevert(bytes("DeployRelay: set DEPLOYMENT_NAME"));
        script.deploymentName(ZCASH_TESTNET, "");
        vm.expectRevert(bytes("DeployRelay: set DEPLOYMENT_NAME"));
        script.deploymentName(ZCASH_MAINNET, "");
    }

    /// @notice ZIP 259: an NU7 activation height is a multiple of 3; `NEVER` stands for "not assigned yet".
    function test_config_nu7ActivationHeightCheck() public {
        DeployRelay script = new DeployRelay();
        script.checkNu7ActivationHeight(4_465_026); // Testnet
        script.checkNu7ActivationHeight(type(uint32).max);
        (ZcashHeaderRelay.RelayParams memory p,,) = script.loadParams("config/relay-testnet.json");
        script.checkNu7ActivationHeight(p.nu7ActivationHeight);
        (p,,) = script.loadParams("config/relay-mainnet.json");
        script.checkNu7ActivationHeight(p.nu7ActivationHeight);
        vm.expectRevert(bytes("DeployRelay: the NU7 activation height is not a multiple of 3 (ZIP 259)"));
        script.checkNu7ActivationHeight(4_465_027);
    }

    function _deployFromConfig(DeployRelay script, string memory config, string memory fixture) internal {
        (ZcashHeaderRelay.RelayParams memory p,, string memory display) = script.loadParams(config);
        DeployRelay.Checkpoint memory cp = script.loadCheckpoint(fixture, display);
        ZcashHeaderRelay relay = new ZcashHeaderRelay(p, cp.hash, cp.height, cp.times, cp.bits);
        RealFixture memory fx = _loadReal(fixture);
        _warpTo(fx.headers[0]);
        assertEq(relay.submitHeader(fx.headers[0]), fx.hashes[0]);
    }

    // --- helpers ------------------------------------------------------------------------------------------------

    function _submitFirst(ZcashHeaderRelay relay, RealFixture memory fx, uint256 count) internal {
        for (uint256 i; i < count; ++i) {
            _warpTo(fx.headers[i]);
            relay.submitHeader(fx.headers[i]);
        }
    }

    function _medianOfLast11(uint32[] memory times) internal pure returns (uint256) {
        uint256[] memory last = new uint256[](11);
        for (uint256 i; i < 11; ++i) {
            last[i] = times[times.length - 1 - i];
        }
        return ZcashPow.median(last);
    }
}
