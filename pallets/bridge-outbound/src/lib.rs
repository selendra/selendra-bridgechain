//! # Bridge Outbound Pallet
//!
//! Outbound message queue for the Substrate → Ethereum bridge.
//!
//! ## How it ties into BEEFY
//!
//! On every block, this pallet collects messages submitted via `submit()` and
//! exposes a single **keccak256 Merkle root** of those messages as the BEEFY-MMR
//! leaf's `leaf_extra` field. Because that leaf is hashed into the MMR root,
//! and the MMR root is what BEEFY validators sign, a relayer can prove a single
//! outbound message on Ethereum with two stacked Merkle proofs:
//!
//!  1. an MMR proof that the BEEFY-MMR leaf for block N is in the signed MMR root,
//!  2. a per-block Merkle proof that a specific message is in `leaf_extra`.
//!
//! Both proofs use keccak256, which Ethereum can verify with a single opcode.
//!
//! ## Timing
//!
//! `pallet-mmr` appends a leaf during `on_initialize` of block N, and that leaf
//! describes block **N-1** (its `parent_number_and_hash`). So our
//! `BeefyDataProvider::extra_data()` returns the commitment for block N-1, not
//! N. Messages submitted in block N are pinned to the MMR at the start of block
//! N+1.

#![cfg_attr(not(feature = "std"), no_std)]

extern crate alloc;

use alloc::vec::Vec;
use codec::{Decode, Encode, MaxEncodedLen};
use frame_support::{pallet_prelude::*, BoundedVec};
use frame_system::pallet_prelude::*;
use scale_info::TypeInfo;
use sp_core::{H160, H256};
use sp_runtime::{
	traits::{Hash, Keccak256, One, Saturating},
	RuntimeDebug,
};

pub use pallet::*;

/// A message destined for an Ethereum-side dispatcher.
///
/// The leaf encoded into our per-block Merkle tree is `keccak256(SCALE(Message))`.
/// The relayer recomputes this on the Ethereum side to prove a specific message.
#[derive(Clone, Eq, PartialEq, Encode, Decode, RuntimeDebug, TypeInfo, MaxEncodedLen)]
#[scale_info(skip_type_params(MaxPayload))]
pub struct Message<MaxPayload: Get<u32>> {
	/// Chain-wide monotonic nonce. Each successful `submit()` increments this.
	pub nonce: u64,
	/// Destination Ethereum contract address.
	pub destination: H160,
	/// Opaque payload — typically ABI-encoded calldata for `destination`.
	pub payload: BoundedVec<u8, MaxPayload>,
}

/// Runtime-API-friendly view of a message, without the `Get`-generic payload bound.
#[derive(Clone, Eq, PartialEq, Encode, Decode, RuntimeDebug, TypeInfo)]
pub struct OutboundMessage {
	pub nonce: u64,
	pub destination: H160,
	pub payload: Vec<u8>,
}

impl<M: Get<u32>> From<Message<M>> for OutboundMessage {
	fn from(m: Message<M>) -> Self {
		Self { nonce: m.nonce, destination: m.destination, payload: m.payload.into_inner() }
	}
}

/// A Merkle proof of a single message inside its per-block tree.
///
/// The proven leaf hash is `keccak256(leaf)`; pair siblings hash up to `root`.
/// `root` should match what the BEEFY-MMR leaf's `leaf_extra` carries.
#[derive(Clone, Eq, PartialEq, Encode, Decode, RuntimeDebug, TypeInfo)]
pub struct OutboundMessageProof {
	pub root: H256,
	pub proof: Vec<H256>,
	pub leaf_count: u32,
	pub leaf_index: u32,
	pub leaf: Vec<u8>,
}

#[frame_support::pallet]
pub mod pallet {
	use super::*;

	#[pallet::config]
	pub trait Config: frame_system::Config {
		type RuntimeEvent: From<Event<Self>>
			+ IsType<<Self as frame_system::Config>::RuntimeEvent>;

		/// Maximum payload size per message, in bytes.
		#[pallet::constant]
		type MaxPayloadSize: Get<u32>;

		/// Maximum messages that can be queued in a single block.
		#[pallet::constant]
		type MaxMessagesPerBlock: Get<u32>;
	}

	#[pallet::pallet]
	pub struct Pallet<T>(_);

	/// Chain-wide monotonic message nonce.
	#[pallet::storage]
	pub type Nonce<T: Config> = StorageValue<_, u64, ValueQuery>;

	/// Messages submitted in each block. Append-only — relayers need history.
	#[pallet::storage]
	pub type Messages<T: Config> = StorageMap<
		_,
		Twox64Concat,
		BlockNumberFor<T>,
		BoundedVec<Message<T::MaxPayloadSize>, T::MaxMessagesPerBlock>,
		ValueQuery,
	>;

	/// Per-block keccak256 Merkle root over the messages. Cached when the
	/// BEEFY-MMR data provider runs so the runtime API can serve historical
	/// lookups without recomputing the tree.
	#[pallet::storage]
	pub type CommitmentRoot<T: Config> =
		StorageMap<_, Twox64Concat, BlockNumberFor<T>, H256, OptionQuery>;

	#[pallet::event]
	#[pallet::generate_deposit(pub(super) fn deposit_event)]
	pub enum Event<T: Config> {
		/// A new outbound message has been queued.
		MessageQueued { nonce: u64, destination: H160, payload_hash: H256 },
	}

	#[pallet::error]
	pub enum Error<T> {
		/// Per-block message limit reached.
		TooManyMessagesThisBlock,
	}

	#[pallet::call]
	impl<T: Config> Pallet<T> {
		/// Queue a message for delivery to an Ethereum contract.
		#[pallet::call_index(0)]
		#[pallet::weight(
			Weight::from_parts(40_000_000, 0)
				.saturating_add(T::DbWeight::get().reads_writes(2, 3))
		)]
		pub fn submit(
			origin: OriginFor<T>,
			destination: H160,
			payload: BoundedVec<u8, T::MaxPayloadSize>,
		) -> DispatchResult {
			let _who = ensure_signed(origin)?;

			let nonce = Nonce::<T>::mutate(|n| {
				*n = n.saturating_add(1);
				*n
			});

			let payload_hash = <Keccak256 as Hash>::hash(payload.as_slice());
			let message = Message { nonce, destination, payload };

			let now = frame_system::Pallet::<T>::block_number();
			Messages::<T>::try_mutate(now, |msgs| msgs.try_push(message))
				.map_err(|_| Error::<T>::TooManyMessagesThisBlock)?;

			Self::deposit_event(Event::MessageQueued { nonce, destination, payload_hash });
			Ok(())
		}
	}

	impl<T: Config> Pallet<T> {
		/// Compute the keccak256 Merkle root of `block`'s messages.
		///
		/// Each leaf is `keccak256(SCALE(Message))`. Returns `None` when the
		/// block had no outbound messages.
		pub fn commitment_root_for(block: BlockNumberFor<T>) -> Option<H256> {
			let messages = Messages::<T>::get(block);
			if messages.is_empty() {
				return None;
			}
			let leaves: Vec<Vec<u8>> = messages.iter().map(|m| m.encode()).collect();
			Some(binary_merkle_tree::merkle_root::<Keccak256, _>(leaves))
		}

		/// Build a Merkle proof for the `index`-th message in `block`'s tree.
		///
		/// Returns `None` for empty blocks or out-of-range indices. The relayer
		/// pairs this with an MMR proof of the BEEFY-MMR leaf to verify a single
		/// message under the BEEFY-signed root on Ethereum.
		pub fn message_proof(
			block: BlockNumberFor<T>,
			index: u32,
		) -> Option<OutboundMessageProof> {
			let messages = Messages::<T>::get(block);
			if messages.is_empty() || (index as usize) >= messages.len() {
				return None;
			}
			let leaves: Vec<Vec<u8>> = messages.iter().map(|m| m.encode()).collect();
			let proof =
				binary_merkle_tree::merkle_proof::<Keccak256, _, _>(leaves, index);
			Some(OutboundMessageProof {
				root: proof.root,
				proof: proof.proof,
				leaf_count: proof.number_of_leaves,
				leaf_index: proof.leaf_index,
				leaf: proof.leaf,
			})
		}

		/// Iterate all messages submitted in `block` as runtime-API views.
		pub fn messages_view(block: BlockNumberFor<T>) -> Vec<OutboundMessage> {
			Messages::<T>::get(block).into_iter().map(Into::into).collect()
		}
	}
}

sp_api::decl_runtime_apis! {
	/// Read access to the outbound bridge state. Relayers use this to fetch
	/// messages and proofs to submit on Ethereum.
	pub trait BridgeOutboundApi<BlockNumber: codec::Codec> {
		/// All messages submitted in `block`. Empty for blocks with no traffic.
		fn messages_at(block: BlockNumber) -> Vec<OutboundMessage>;

		/// The keccak256 Merkle root pinned into the BEEFY-MMR leaf's `leaf_extra`
		/// for `block`. `None` if the block had no outbound messages.
		fn commitment_root_at(block: BlockNumber) -> Option<H256>;

		/// Proof that the `index`-th message in `block`'s tree is part of the
		/// commitment root. Returned proof is self-contained — caller does not
		/// need to know the tree size or other leaves.
		fn message_proof(block: BlockNumber, index: u32) -> Option<OutboundMessageProof>;
	}
}

/// Plugs into `pallet-beefy-mmr` as the producer of `leaf_extra`.
///
/// `pallet-mmr` appends the leaf for block N during `on_initialize(N)`, and the
/// leaf describes block N-1 — so we return the commitment for the parent. The
/// caching write into `CommitmentRoot` is idempotent: any extra calls within
/// the block recompute the same root.
///
/// Typed as `H256` (32-byte fixed) so the SCALE encoding of the MMR leaf has a
/// flat 32-byte tail with no compact-length prefix. The Ethereum-side
/// `Gateway.hashMmrLeaf` mirrors that layout. Empty blocks emit `H256::zero()`
/// so the leaf is still well-formed for downstream verification.
impl<T: Config> sp_consensus_beefy::mmr::BeefyDataProvider<H256> for Pallet<T> {
	fn extra_data() -> H256 {
		let parent = frame_system::Pallet::<T>::block_number().saturating_sub(One::one());
		match Pallet::<T>::commitment_root_for(parent) {
			Some(root) => {
				CommitmentRoot::<T>::insert(parent, root);
				root
			},
			None => H256::zero(),
		}
	}
}
