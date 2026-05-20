//! A collection of node-specific RPC methods.
//! Substrate provides the `sc-rpc` crate, which defines the core RPC layer
//! used by Substrate nodes. This file extends those RPC definitions with
//! capabilities that are specific to this project's runtime configuration.

#![warn(missing_docs)]

use std::sync::Arc;

use jsonrpsee::RpcModule;
use sc_client_api::{AuxStore, Backend, StorageProvider};
use sc_consensus_beefy::communication::notification::{
	BeefyBestBlockStream, BeefyVersionedFinalityProofStream,
};
use sc_consensus_beefy_rpc::{Beefy, BeefyApiServer};
use sc_rpc::SubscriptionTaskExecutor;
use sc_transaction_pool_api::TransactionPool;
use solochain_template_runtime::{opaque::Block, AccountId, Balance, BlockNumber, Nonce};
use sp_api::ProvideRuntimeApi;
use sp_block_builder::BlockBuilder;
use sp_blockchain::{Error as BlockChainError, HeaderBackend, HeaderMetadata};
use sp_consensus_beefy::ecdsa_crypto::AuthorityId as BeefyId;

/// Streams + executor that the BEEFY RPC subscription endpoints need.
pub struct BeefyDeps {
	/// Stream of signed BEEFY commitments emitted by the voter.
	pub beefy_finality_proof_stream: BeefyVersionedFinalityProofStream<Block, BeefyId>,
	/// Stream of best BEEFY block hashes as the voter advances.
	pub beefy_best_block_stream: BeefyBestBlockStream<Block>,
	/// Executor used to drive subscription tasks.
	pub subscription_executor: SubscriptionTaskExecutor,
}

/// Full client dependencies.
pub struct FullDeps<C, P, B> {
	/// The client instance to use.
	pub client: Arc<C>,
	/// Transaction pool instance.
	pub pool: Arc<P>,
	/// The chain backend — `mmr_rpc` needs its offchain storage to build
	/// MMR proofs from the indexed leaves.
	pub backend: Arc<B>,
	/// BEEFY-specific dependencies.
	pub beefy: BeefyDeps,
}

/// Instantiate all full RPC extensions.
pub fn create_full<C, P, B>(
	deps: FullDeps<C, P, B>,
) -> Result<RpcModule<()>, Box<dyn std::error::Error + Send + Sync>>
where
	C: ProvideRuntimeApi<Block>,
	C: HeaderBackend<Block> + HeaderMetadata<Block, Error = BlockChainError> + 'static,
	C: AuxStore + StorageProvider<Block, B> + Send + Sync + 'static,
	C::Api: substrate_frame_rpc_system::AccountNonceApi<Block, AccountId, Nonce>,
	C::Api: pallet_transaction_payment_rpc::TransactionPaymentRuntimeApi<Block, Balance>,
	C::Api: BlockBuilder<Block>,
	C::Api: mmr_rpc::MmrRuntimeApi<Block, <Block as sp_runtime::traits::Block>::Hash, BlockNumber>,
	P: TransactionPool + 'static,
	B: Backend<Block> + Send + Sync + 'static,
{
	use mmr_rpc::{Mmr, MmrApiServer};
	use pallet_transaction_payment_rpc::{TransactionPayment, TransactionPaymentApiServer};
	use substrate_frame_rpc_system::{System, SystemApiServer};

	let mut module = RpcModule::new(());
	let FullDeps { client, pool, backend, beefy } = deps;

	module.merge(System::new(client.clone(), pool).into_rpc())?;
	module.merge(TransactionPayment::new(client.clone()).into_rpc())?;

	// MMR proof generation needs the offchain-indexed leaves the runtime
	// writes during `on_finalize`. Without this, `mmr_generateProof` won't
	// be registered and any relayer trying to fetch a proof gets a method-
	// not-found.
	let offchain = backend
		.offchain_storage()
		.ok_or("MMR RPC requires offchain storage support from the backend")?;
	module.merge(Mmr::new(client, offchain).into_rpc())?;

	module.merge(
		Beefy::<Block, BeefyId>::new(
			beefy.beefy_finality_proof_stream,
			beefy.beefy_best_block_stream,
			beefy.subscription_executor,
		)?
		.into_rpc(),
	)?;

	Ok(module)
}
