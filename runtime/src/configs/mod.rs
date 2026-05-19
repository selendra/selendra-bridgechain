// This is free and unencumbered software released into the public domain.
//
// Anyone is free to copy, modify, publish, use, compile, sell, or
// distribute this software, either in source code form or as a compiled
// binary, for any purpose, commercial or non-commercial, and by any
// means.
//
// In jurisdictions that recognize copyright laws, the author or authors
// of this software dedicate any and all copyright interest in the
// software to the public domain. We make this dedication for the benefit
// of the public at large and to the detriment of our heirs and
// successors. We intend this dedication to be an overt act of
// relinquishment in perpetuity of all present and future rights to this
// software under copyright law.
//
// THE SOFTWARE IS PROVIDED "AS IS", WITHOUT WARRANTY OF ANY KIND,
// EXPRESS OR IMPLIED, INCLUDING BUT NOT LIMITED TO THE WARRANTIES OF
// MERCHANTABILITY, FITNESS FOR A PARTICULAR PURPOSE AND NONINFRINGEMENT.
// IN NO EVENT SHALL THE AUTHORS BE LIABLE FOR ANY CLAIM, DAMAGES OR
// OTHER LIABILITY, WHETHER IN AN ACTION OF CONTRACT, TORT OR OTHERWISE,
// ARISING FROM, OUT OF OR IN CONNECTION WITH THE SOFTWARE OR THE USE OR
// OTHER DEALINGS IN THE SOFTWARE.
//
// For more information, please refer to <http://unlicense.org>

// Substrate and Polkadot dependencies
use frame_support::{
	derive_impl, parameter_types,
	traits::{ConstBool, ConstU128, ConstU32, ConstU64, ConstU8, VariantCountOf},
	weights::{
		constants::{RocksDbWeight, WEIGHT_REF_TIME_PER_SECOND},
		IdentityFee, Weight,
	},
};
use frame_system::limits::{BlockLength, BlockWeights};
use pallet_transaction_payment::{ConstFeeMultiplier, FungibleAdapter, Multiplier};
use sp_consensus_aura::sr25519::AuthorityId as AuraId;
use sp_consensus_beefy::{ecdsa_crypto::AuthorityId as BeefyId, mmr::MmrLeafVersion};
use sp_runtime::{
	traits::{ConvertInto, Keccak256, One, OpaqueKeys},
	Perbill,
};
use sp_version::RuntimeVersion;

// Local module imports
use super::{
	AccountId, Aura, Balance, Balances, BeefyMmrLeaf, Block, BlockNumber, BridgeOutbound, Hash,
	Nonce, PalletInfo, Runtime, RuntimeCall, RuntimeEvent, RuntimeFreezeReason, RuntimeHoldReason,
	RuntimeOrigin, RuntimeTask, SessionKeys, System, DAYS, EXISTENTIAL_DEPOSIT, SLOT_DURATION,
	VERSION,
};

const NORMAL_DISPATCH_RATIO: Perbill = Perbill::from_percent(75);

parameter_types! {
	pub const BlockHashCount: BlockNumber = 2400;
	pub const Version: RuntimeVersion = VERSION;

	/// We allow for 2 seconds of compute with a 6 second average block time.
	pub RuntimeBlockWeights: BlockWeights = BlockWeights::with_sensible_defaults(
		Weight::from_parts(2u64 * WEIGHT_REF_TIME_PER_SECOND, u64::MAX),
		NORMAL_DISPATCH_RATIO,
	);
	pub RuntimeBlockLength: BlockLength = BlockLength::max_with_normal_ratio(5 * 1024 * 1024, NORMAL_DISPATCH_RATIO);
	pub const SS58Prefix: u8 = 42;
}

/// The default types are being injected by [`derive_impl`](`frame_support::derive_impl`) from
/// [`SoloChainDefaultConfig`](`struct@frame_system::config_preludes::SolochainDefaultConfig`),
/// but overridden as needed.
#[derive_impl(frame_system::config_preludes::SolochainDefaultConfig)]
impl frame_system::Config for Runtime {
	/// The block type for the runtime.
	type Block = Block;
	/// Block & extrinsics weights: base values and limits.
	type BlockWeights = RuntimeBlockWeights;
	/// The maximum length of a block (in bytes).
	type BlockLength = RuntimeBlockLength;
	/// The identifier used to distinguish between accounts.
	type AccountId = AccountId;
	/// The type for storing how many extrinsics an account has signed.
	type Nonce = Nonce;
	/// The type for hashing blocks and tries.
	type Hash = Hash;
	/// Maximum number of block number to block hash mappings to keep (oldest pruned first).
	type BlockHashCount = BlockHashCount;
	/// The weight of database operations that the runtime can invoke.
	type DbWeight = RocksDbWeight;
	/// Version of the runtime.
	type Version = Version;
	/// The data to be stored in an account.
	type AccountData = pallet_balances::AccountData<Balance>;
	/// This is used as an identifier of the chain. 42 is the generic substrate prefix.
	type SS58Prefix = SS58Prefix;
	type MaxConsumers = frame_support::traits::ConstU32<16>;
}

impl pallet_aura::Config for Runtime {
	type AuthorityId = AuraId;
	type DisabledValidators = ();
	type MaxAuthorities = ConstU32<32>;
	type AllowMultipleBlocksPerSlot = ConstBool<false>;
	type SlotDuration = pallet_aura::MinimumPeriodTimesTwo<Runtime>;
}

impl pallet_grandpa::Config for Runtime {
	type RuntimeEvent = RuntimeEvent;

	type WeightInfo = ();
	type MaxAuthorities = ConstU32<32>;
	type MaxNominators = ConstU32<0>;
	type MaxSetIdSessionEntries = ConstU64<0>;

	type KeyOwnerProof = sp_core::Void;
	type EquivocationReportSystem = ();
}

impl pallet_timestamp::Config for Runtime {
	/// A timestamp: milliseconds since the unix epoch.
	type Moment = u64;
	type OnTimestampSet = Aura;
	type MinimumPeriod = ConstU64<{ SLOT_DURATION / 2 }>;
	type WeightInfo = ();
}

impl pallet_balances::Config for Runtime {
	type MaxLocks = ConstU32<50>;
	type MaxReserves = ();
	type ReserveIdentifier = [u8; 8];
	/// The type for recording an account's balance.
	type Balance = Balance;
	/// The ubiquitous event type.
	type RuntimeEvent = RuntimeEvent;
	type DustRemoval = ();
	type ExistentialDeposit = ConstU128<EXISTENTIAL_DEPOSIT>;
	type AccountStore = System;
	type WeightInfo = pallet_balances::weights::SubstrateWeight<Runtime>;
	type FreezeIdentifier = RuntimeFreezeReason;
	type MaxFreezes = VariantCountOf<RuntimeFreezeReason>;
	type RuntimeHoldReason = RuntimeHoldReason;
	type RuntimeFreezeReason = RuntimeFreezeReason;
	type DoneSlashHandler = ();
}

parameter_types! {
	pub FeeMultiplier: Multiplier = Multiplier::one();
}

impl pallet_transaction_payment::Config for Runtime {
	type RuntimeEvent = RuntimeEvent;
	type OnChargeTransaction = FungibleAdapter<Balances, ()>;
	type OperationalFeeMultiplier = ConstU8<5>;
	type WeightToFee = IdentityFee<Balance>;
	type LengthToFee = IdentityFee<Balance>;
	type FeeMultiplierUpdate = ConstFeeMultiplier<FeeMultiplier>;
	type WeightInfo = pallet_transaction_payment::weights::SubstrateWeight<Runtime>;
}

impl pallet_sudo::Config for Runtime {
	type RuntimeEvent = RuntimeEvent;
	type RuntimeCall = RuntimeCall;
	type WeightInfo = pallet_sudo::weights::SubstrateWeight<Runtime>;
}

/// Configure the pallet-template in pallets/template.
impl pallet_template::Config for Runtime {
	type RuntimeEvent = RuntimeEvent;
	type WeightInfo = pallet_template::weights::SubstrateWeight<Runtime>;
}

// =====================================================================
// BEEFY + MMR stack — backs the Ethereum bridge light client.
// =====================================================================
//
// Architecture:
//   pallet-session is the validator-set notifier. It drives `OneSessionHandler`s
//   (Aura, Grandpa, Beefy) so they all stay in sync with the active authority set.
//   pallet-beefy attests to MMR roots; pallet-beefy-mmr extends the leaf with the
//   BEEFY-next-authority-set merkle root so Ethereum can verify validator rotation.
//
// MVP simplifications:
//   * No staking / no slashing — `EquivocationReportSystem = ()`.
//   * Sessions never rotate on their own (`PeriodicSessions` with a 1-year period);
//     the static authority set persists across the chain's life unless changed by
//     sudo. This is acceptable for an MVP; production must wire pallet-staking.

parameter_types! {
	pub const Period: BlockNumber = 365 * DAYS;
	pub const Offset: BlockNumber = 0;
	// Leaf version 0.0 — single-byte version, minor=0, major=0.
	pub LeafVersion: MmrLeafVersion = MmrLeafVersion::new(0, 0);
}

impl pallet_session::Config for Runtime {
	type RuntimeEvent = RuntimeEvent;
	type ValidatorId = AccountId;
	type ValidatorIdOf = ConvertInto;
	type ShouldEndSession = pallet_session::PeriodicSessions<Period, Offset>;
	type NextSessionRotation = pallet_session::PeriodicSessions<Period, Offset>;
	type SessionManager = ();
	type SessionHandler = <SessionKeys as OpaqueKeys>::KeyTypeIdProviders;
	type Keys = SessionKeys;
	type DisablingStrategy = ();
	type WeightInfo = ();
}

impl pallet_beefy::Config for Runtime {
	type BeefyId = BeefyId;
	type MaxAuthorities = ConstU32<32>;
	type MaxNominators = ConstU32<0>;
	type MaxSetIdSessionEntries = ConstU64<0>;
	type OnNewValidatorSet = BeefyMmrLeaf;
	type AncestryHelper = BeefyMmrLeaf;
	type WeightInfo = ();
	type KeyOwnerProof = sp_core::Void;
	type EquivocationReportSystem = ();
}

impl pallet_mmr::Config for Runtime {
	const INDEXING_PREFIX: &'static [u8] = b"mmr";
	// Critical for the Ethereum bridge: keccak256 is the only hash a Solidity
	// contract can verify cheaply (single EVM opcode). BlakeTwo256 would be
	// catastrophically expensive on-chain.
	type Hashing = Keccak256;
	type OnNewRoot = pallet_beefy_mmr::DepositBeefyDigest<Runtime>;
	type LeafData = pallet_beefy_mmr::Pallet<Runtime>;
	type BlockHashProvider = pallet_mmr::DefaultBlockHashProvider<Runtime>;
	type WeightInfo = ();
	#[cfg(feature = "runtime-benchmarks")]
	type BenchmarkHelper = ();
}

impl pallet_beefy_mmr::Config for Runtime {
	type LeafVersion = LeafVersion;
	// secp256k1 pubkey → 20-byte Ethereum address. Lets the Solidity verifier
	// recover signatures with `ecrecover` rather than running secp256k1
	// curve math on-chain.
	type BeefyAuthorityToMerkleLeaf = pallet_beefy_mmr::BeefyEcdsaToEthereum;
	// `leaf_extra` carries the per-block outbound-commitment root: keccak256
	// Merkle root of the messages submitted in the parent block. Verifying a
	// message on Ethereum becomes (MMR proof of leaf) + (Merkle proof under
	// `leaf_extra`).
	type LeafExtra = alloc::vec::Vec<u8>;
	type BeefyDataProvider = BridgeOutbound;
	type WeightInfo = ();
}

// =====================================================================
// Outbound side of the Ethereum bridge.
// =====================================================================

parameter_types! {
	// 4 KiB matches the rough Ethereum calldata-cost sweet spot — bigger
	// payloads aren't impossible but cost more gas on submit.
	pub const MaxOutboundPayloadSize: u32 = 4 * 1024;
	// 256 messages/block is way above realistic throughput for an MVP; bump
	// later if you're seeing TooManyMessagesThisBlock in the wild.
	pub const MaxOutboundMessagesPerBlock: u32 = 256;
}

impl pallet_bridge_outbound::Config for Runtime {
	type RuntimeEvent = RuntimeEvent;
	type MaxPayloadSize = MaxOutboundPayloadSize;
	type MaxMessagesPerBlock = MaxOutboundMessagesPerBlock;
}

// =====================================================================
// Inbound side of the Ethereum bridge.
// =====================================================================

parameter_types! {
	pub const MaxInboundPayloadSize: u32 = 4 * 1024;
	// MVP: a placeholder gateway address. Replace before any real deployment
	// with the deployed `Gateway.sol` contract address on the target network.
	pub const GatewayAddress: sp_core::H160 = sp_core::H160([0u8; 20]);
}

impl pallet_bridge_inbound::Config for Runtime {
	type RuntimeEvent = RuntimeEvent;
	// `MockVerifier` accepts any proof. Swap this to the vendored
	// `snowbridge-pallet-ethereum-client` impl when it lands. The trait surface
	// is what matters — the rest of this pallet doesn't change.
	type Verifier = pallet_bridge_inbound::MockVerifier;
	// No application pallets to route into yet; the dispatch is a no-op that
	// still records nonces and emits MessageReceived events.
	type MessageDispatch = pallet_bridge_inbound::NoopDispatch;
	type MaxPayloadSize = MaxInboundPayloadSize;
	type GatewayAddress = GatewayAddress;
}
