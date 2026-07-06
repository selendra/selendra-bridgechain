//! The Borsh instruction wire format a Solana gate program (de)serializes.
//!
//! Solana programs take a raw byte buffer as instruction data; the convention is
//! Borsh. This is the on-the-wire contract between the off-chain keeper (which
//! builds a `Claim`) and the on-chain program (which decodes and executes it).
//! Governance instructions are included for completeness; the bridge hot path is
//! `Send` (source) and `Claim` (target).

use borsh::{BorshDeserialize, BorshSerialize};

/// Execution payload, Borsh-encoded (Solana-native form of `AutoParamsTo`).
#[derive(BorshSerialize, BorshDeserialize, Clone, Debug, Default, PartialEq, Eq)]
pub struct AutoParamsWire {
    pub execution_fee: u128,
    pub flags: u64,
    pub fallback_address: Vec<u8>,
    pub data: Vec<u8>,
}

#[derive(BorshSerialize, BorshDeserialize, Clone, Debug, PartialEq, Eq)]
pub struct SendArgs {
    pub debridge_id: [u8; 32],
    pub amount: u64,
    pub chain_id_to: u64,
    /// 20-byte EVM address (Solana→EVM).
    pub receiver: Vec<u8>,
    pub auto: Option<AutoParamsWire>,
}

#[derive(BorshSerialize, BorshDeserialize, Clone, Debug, PartialEq, Eq)]
pub struct ClaimArgs {
    pub debridge_id: [u8; 32],
    pub amount: u64,
    pub chain_id_from: u64,
    pub nonce: u64,
    /// 32-byte Solana token account (EVM→Solana).
    pub receiver: Vec<u8>,
    pub auto: Option<AutoParamsWire>,
    /// Packed source-chain sender; needed to recompute the id when `auto` is set.
    pub native_sender: Vec<u8>,
    /// 65-byte `r||s||v` validator signatures, sorted ascending by signer.
    pub signatures: Vec<Vec<u8>>,
}

#[derive(BorshSerialize, BorshDeserialize, Clone, Debug, PartialEq, Eq)]
pub struct InitArgs {
    /// EVM validator addresses (the same set the EVM gate trusts).
    pub validators: Vec<[u8; 20]>,
    pub threshold: u32,
    /// This gate's chain id (Solana).
    pub chain_id: u64,
}

#[derive(BorshSerialize, BorshDeserialize, Clone, Debug, PartialEq, Eq)]
pub enum GateInstruction {
    /// Create + populate the Config PDA (validator set, threshold, chain id).
    Init(InitArgs),
    Send(SendArgs),
    Claim(ClaimArgs),
    SetValidator { validator: [u8; 20], active: bool },
    SetThreshold { threshold: u32 },
}

impl GateInstruction {
    pub fn to_bytes(&self) -> Vec<u8> {
        borsh::to_vec(self).expect("borsh serialize GateInstruction")
    }

    pub fn try_from_bytes(data: &[u8]) -> std::io::Result<Self> {
        borsh::from_slice(data)
    }
}
