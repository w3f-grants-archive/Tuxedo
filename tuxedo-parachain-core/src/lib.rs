//! This module is the core of Tuxedo's parachain support.
//!
//! The types and methods defined in this crate are of equal importance to
//! those in the `tuxedo-core` crate, and this crate should be considered
//! a simple extension to that one and equally "core"y. The reason Tuxedo
//! separates the parachain specific aspects is because Polkadot and Cumulus
//! are quite heavy to compile, and sovereign chains are able to completely avoid it.
//!
//! It's primary jobs are to
//! * Manage transient storage details for the parachain inherent, specifically the relay
//!   parent block number.
//! * Provide collation information to the client side collator service.
//! * Implement the `validate_block` function required by relay chain validators.
//!   This task is achieved through the `register_validate_block!` macro.
//!
//! This code is inspired by, cumulus pallet parachain system
//! https://paritytech.github.io/polkadot-sdk/master/cumulus_pallet_parachain_system/index.html

#![cfg_attr(not(feature = "std"), no_std)]

#[cfg(test)]
mod tests;
#[cfg(not(feature = "std"))]
#[doc(hidden)]
pub mod validate_block;

mod collation_api;
mod relay_state_snapshot;
pub use collation_api::ParachainExecutiveExtension;
use parity_scale_codec::{Decode, Encode};

#[cfg(not(feature = "std"))]
#[doc(hidden)]
mod trie_cache;

#[cfg(not(feature = "std"))]
#[doc(hidden)]
pub use bytes;
#[cfg(not(feature = "std"))]
#[doc(hidden)]
pub use parity_scale_codec::decode_from_bytes;
#[cfg(not(feature = "std"))]
#[doc(hidden)]
pub use polkadot_parachain_primitives;
#[cfg(not(feature = "std"))]
#[doc(hidden)]
pub use sp_runtime::traits::GetRuntimeBlockType;
#[cfg(not(feature = "std"))]
#[doc(hidden)]
pub use sp_std;

/// Re-export of the Tuxedo-core crate. This allows parachain-specific
/// Tuxedo-pieces to depend only on tuxedo-parachain-core without worrying about
/// accidental version mismatches.
pub use tuxedo_core;

use cumulus_primitives_parachain_inherent::ParachainInherentData;
use tuxedo_core::{
    dynamic_typing::UtxoData,
    support_macros::{CloneNoBound, DebugNoBound},
    ConstraintChecker,
};

/// A transient storage key that will hold the block number of the relay chain parent
/// that is associated with the current parachain block. This data enters the parachain
/// through the parachain inherent
const RELAY_PARENT_NUMBER_KEY: &[u8] = b"relay_parent_number";

/// An abstraction over reading the ambiently available relay parent block number.
/// This allows it to be mocked during tests and not require actual externalities.
pub trait GetRelayParentNumberStorage {
    fn get() -> u32;
}

/// An abstraction over setting the ambiently available relay parent block number.
/// This allows it to be mocked during tests and require actual externalities.
pub trait SetRelayParentNumberStorage {
    fn set(new_parent_number: u32);
}

/// A public interface for accessing and mutating the relay parent number. This is
/// expected to be called from the parachain piece
pub enum RelayParentNumberStorage {}

impl GetRelayParentNumberStorage for RelayParentNumberStorage {
    fn get() -> u32 {
        let encoded = sp_io::storage::get(RELAY_PARENT_NUMBER_KEY)
            .expect("Some relay parent number should always be stored");
        Decode::decode(&mut &encoded[..])
            .expect("properly encoded relay parent number should have been stored.")
    }
}

impl SetRelayParentNumberStorage for RelayParentNumberStorage {
    fn set(new_parent_number: u32) {
        sp_io::storage::set(RELAY_PARENT_NUMBER_KEY, &new_parent_number.encode());
    }
}

/// A mock version of the RelayParentNumberStorage that can be used in tests without externalities.
pub enum MockRelayParentNumberStorage {}

impl SetRelayParentNumberStorage for MockRelayParentNumberStorage {
    fn set(_new_parent_number: u32) {}
}

/// Basically the same as
/// [`ValidationParams`](polkadot_parachain_primitives::primitives::ValidationParams), but a little
/// bit optimized for our use case here.
///
/// `block_data` and `head_data` are represented as [`bytes::Bytes`] to make them reuse
/// the memory of the input parameter of the exported `validate_blocks` function.
///
/// The layout of this type must match exactly the layout of
/// [`ValidationParams`](polkadot_parachain_primitives::primitives::ValidationParams) to have the
/// same SCALE encoding.
#[derive(parity_scale_codec::Decode)]
#[cfg_attr(feature = "std", derive(parity_scale_codec::Encode))]
#[doc(hidden)]
pub struct MemoryOptimizedValidationParams {
    pub parent_head: bytes::Bytes,
    pub block_data: bytes::Bytes,
    pub relay_parent_number: cumulus_primitives_core::relay_chain::BlockNumber,
    pub relay_parent_storage_root: cumulus_primitives_core::relay_chain::Hash,
}

/// Prepares a Tuxedo runtime to be parachain compatible by doing two main tasks.
///
/// 1. Wraps the provided constraint checker in another layer of aggregation including the parachain
///    inherent piece
/// 2. Registers the `validate_block` function that is used by parachains to validate blocks on a
///    validator when building to wasm. This is skipped when building to std.
///
/// Expects as parameters a Verifier, a non-yet-parachain-ready ConstraintChecker, and a ParaId.
pub use tuxedo_parachainify::parachainify;

// Having to do this wrapping is one more reason to abandon this UtxoData trait,
// and go for a more strongly typed aggregate type approach.
// Tracking issue: https://github.com/Off-Narrative-Labs/Tuxedo/issues/153
/// A wrapper type around Cumulus's ParachainInherentData type that can be stored.
#[derive(Encode, Decode, DebugNoBound, CloneNoBound, scale_info::TypeInfo)]
/// A wrapper type around Cumulus's ParachainInherentData type.
/// This type is convertible Into and From the inner type.
/// This is necessary so that we can implement the `UtxoData` trait.
pub struct ParachainInherentDataUtxo(ParachainInherentData);

impl UtxoData for ParachainInherentDataUtxo {
    const TYPE_ID: [u8; 4] = *b"para";
}

impl From<ParachainInherentDataUtxo> for ParachainInherentData {
    fn from(val: ParachainInherentDataUtxo) -> Self {
        val.0
    }
}

impl From<ParachainInherentData> for ParachainInherentDataUtxo {
    fn from(value: ParachainInherentData) -> Self {
        Self(value)
    }
}

/// A way for the relay chain validators to determine whether a particular parachain
/// extrinsic is the parachain inherent and whether the parachain inherent data can
/// be extracted from it.
pub trait ParachainConstraintChecker: ConstraintChecker {
    fn is_parachain(&self) -> bool;
}
