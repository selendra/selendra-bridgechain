//! # Bridge Inbound Pallet
//!
//! Inbound message queue for the Ethereum → Substrate direction of the bridge.
//!
//! ## Verification
//!
//! Authenticity of an inbound message is checked through the [`Verifier`]
//! trait. A real implementation walks a beacon-chain SSZ proof against an
//! execution-layer state root stored by `snowbridge-pallet-ethereum-client`.
//! For the MVP this pallet ships [`MockVerifier`], which accepts any input —
//! plug a real verifier into the runtime Config when the beacon-client pallet
//! is online.
//!
//! ## Dispatch
//!
//! After verification, the payload is forwarded to the application via the
//! [`MessageDispatch`] trait. The runtime decides where to route messages.
//!
//! ## Replay protection
//!
//! Each accepted message's nonce is recorded in `SeenNonces`. Submissions
//! reusing a nonce are rejected with `NonceAlreadyUsed`.

#![cfg_attr(not(feature = "std"), no_std)]

extern crate alloc;

use alloc::vec::Vec;
use codec::{Decode, DecodeWithMemTracking, Encode, MaxEncodedLen};
use frame_support::{pallet_prelude::*, BoundedVec};
use frame_system::pallet_prelude::*;
use scale_info::TypeInfo;
use sp_core::{H160, H256};
use sp_runtime::RuntimeDebug;

pub use pallet::*;

/// Decoded Ethereum log as the bridge sees it. Unbounded; only used as a
/// dispatch parameter (not stored), so a `Vec` is fine.
#[derive(Clone, Eq, PartialEq, Encode, Decode, DecodeWithMemTracking, RuntimeDebug, TypeInfo)]
pub struct VerifierLog {
	/// Contract that emitted the log.
	pub address: H160,
	/// Indexed topics (EVM allows up to 4).
	pub topics: Vec<H256>,
	/// Non-indexed data, ABI-encoded.
	pub data: Vec<u8>,
}

/// Reasons a [`Verifier`] may reject a proof.
#[derive(Clone, Encode, Decode, RuntimeDebug, TypeInfo, PartialEq, Eq)]
pub enum VerifierError {
	InvalidProof,
	NotFinalized,
	HeaderNotFound,
}

/// Anything that can attest an Ethereum log came from a finalized Ethereum
/// block on the canonical chain.
pub trait Verifier {
	/// Opaque proof bytes — format defined by the implementer.
	type Proof: Parameter + MaxEncodedLen;

	fn verify(log: &VerifierLog, proof: &Self::Proof) -> Result<(), VerifierError>;
}

/// MVP verifier that accepts every input. The real `EthereumClient` pallet
/// will satisfy this trait once it's vendored and wired up.
///
/// ⚠️ Anyone can submit any log when this is in use. Don't ship to production.
pub struct MockVerifier;
impl Verifier for MockVerifier {
	type Proof = BoundedVec<u8, ConstU32<8192>>;

	fn verify(_log: &VerifierLog, _proof: &Self::Proof) -> Result<(), VerifierError> {
		Ok(())
	}
}

/// Application hook — runs after a message has been verified.
pub trait MessageDispatch {
	/// Dispatch the verified payload.
	///
	/// `origin` is the Ethereum contract that produced the message (the
	/// configured Gateway by the time this is called). Implementations should
	/// not assume the payload is well-formed and must validate it.
	fn dispatch(origin: H160, payload: &[u8]) -> DispatchResult;
}

/// No-op dispatch. Useful before any application pallets are wired in: the
/// event still fires and replay protection still works.
pub struct NoopDispatch;
impl MessageDispatch for NoopDispatch {
	fn dispatch(_origin: H160, _payload: &[u8]) -> DispatchResult {
		Ok(())
	}
}

#[frame_support::pallet]
pub mod pallet {
	use super::*;

	#[pallet::config]
	pub trait Config: frame_system::Config {
		type RuntimeEvent: From<Event<Self>>
			+ IsType<<Self as frame_system::Config>::RuntimeEvent>;

		/// Cryptographic verifier for inbound logs.
		type Verifier: Verifier;

		/// Application hook for verified messages.
		type MessageDispatch: MessageDispatch;

		/// Maximum payload size per inbound message.
		#[pallet::constant]
		type MaxPayloadSize: Get<u32>;

		/// Ethereum gateway contract. Logs from other contracts are rejected.
		#[pallet::constant]
		type GatewayAddress: Get<H160>;
	}

	#[pallet::pallet]
	pub struct Pallet<T>(_);

	/// Every accepted nonce — replay protection.
	#[pallet::storage]
	pub type SeenNonces<T: Config> = StorageMap<_, Identity, u64, (), OptionQuery>;

	/// Largest nonce accepted so far. Relayers use this to discover the
	/// catch-up range when re-syncing.
	#[pallet::storage]
	pub type LatestNonce<T: Config> = StorageValue<_, u64, ValueQuery>;

	#[pallet::event]
	#[pallet::generate_deposit(pub(super) fn deposit_event)]
	pub enum Event<T: Config> {
		/// An inbound Ethereum message was verified and dispatched.
		MessageReceived { nonce: u64, origin: H160 },
	}

	#[pallet::error]
	pub enum Error<T> {
		/// Proof did not validate against the verifier.
		InvalidProof,
		/// Log was emitted by a contract other than the configured gateway.
		UnexpectedOrigin,
		/// Nonce was already consumed by a prior submission.
		NonceAlreadyUsed,
		/// MessageDispatch returned an error.
		DispatchFailed,
	}

	#[pallet::call]
	impl<T: Config> Pallet<T> {
		/// Submit a verified Ethereum log plus its decoded envelope.
		///
		/// Anyone can call — the verifier (not the caller's origin) is what
		/// determines authenticity.
		#[pallet::call_index(0)]
		#[pallet::weight(
			Weight::from_parts(80_000_000, 0)
				.saturating_add(T::DbWeight::get().reads_writes(2, 3))
		)]
		pub fn submit(
			origin: OriginFor<T>,
			message_origin: H160,
			nonce: u64,
			payload: BoundedVec<u8, T::MaxPayloadSize>,
			log: VerifierLog,
			proof: <T::Verifier as Verifier>::Proof,
		) -> DispatchResult {
			let _who = ensure_signed(origin)?;

			T::Verifier::verify(&log, &proof).map_err(|_| Error::<T>::InvalidProof)?;

			ensure!(log.address == T::GatewayAddress::get(), Error::<T>::UnexpectedOrigin);

			ensure!(!SeenNonces::<T>::contains_key(nonce), Error::<T>::NonceAlreadyUsed);
			SeenNonces::<T>::insert(nonce, ());
			LatestNonce::<T>::mutate(|n| *n = (*n).max(nonce));

			T::MessageDispatch::dispatch(message_origin, payload.as_slice())
				.map_err(|_| Error::<T>::DispatchFailed)?;

			Self::deposit_event(Event::MessageReceived { nonce, origin: message_origin });
			Ok(())
		}
	}

	impl<T: Config> Pallet<T> {
		/// Largest nonce the chain has accepted so far. Used by the runtime API.
		pub fn latest_nonce() -> u64 {
			LatestNonce::<T>::get()
		}

		/// Whether a nonce has already been consumed.
		pub fn is_nonce_used(nonce: u64) -> bool {
			SeenNonces::<T>::contains_key(nonce)
		}
	}
}

sp_api::decl_runtime_apis! {
	/// Inbound bridge state queries for relayers.
	pub trait BridgeInboundApi {
		/// Largest nonce the chain has seen so far.
		fn latest_nonce() -> u64;

		/// Whether a specific nonce has already been delivered.
		fn is_nonce_used(nonce: u64) -> bool;
	}
}
