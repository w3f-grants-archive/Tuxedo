//! APIs and utilities for working with Substrate's Inherents in Tuxedo based chains.
//!
//! # Substrate inherents
//!
//! Inherents are a Substrate feature that allows block authors to insert some transactions directly
//! into the body of the block. Inherents are similar to pre-runtime digests which allow authors to
//! insert info into the block header. However inherents go in the block body and therefore must be transactions.
//!
//! Classic usecases for inherents are injecting and updating environmental information such as a block timestamp,
//! information about the relay chain (if the current chain is a parachain), or information about who should receive the block reward.
//!
//! In order to allow the runtime to construct such transactions while keeping the cleint opaque, there are special APIs
//! for creating inherents and performing off-chain validation of inherents. That's right, inherents also offer
//! a special API to have their environmental data checked off-chain before the block is executed.
//!
//! # Complexities in UTXO chains
//!
//! In account based systems, the classic way to use an inherent is that the block inserts a transaction providing some data like a timestamp.
//! When the extrinsic executed it, overwrites the previously stored timestamp in a dedicated storage item.
//!
//! In UTXO chains, there are no storage items, and all state is local to a UTXO. This is the case with, for example, the timestamp as well.
//! This means that when the author calls into the runtime with a timestamp, the transaction that is returned must include the correct reference
//! to the UTXO that contained the previous best timestamp. This is the crux of the problem: there is no easy way to know the location of
//! the previous timestamp in the utxo-space from inside the runtime.
//!
//! # Scraping the Parent Block
//!
//! The solution is to provide the entirety of the previous block to the runtime when asking it to construct inherents.
//! This module provides an inherent data provider that does just this. Any Tuxedo runtime that uses inherents (At least ones
//! that update environmental data), needs to include this foundational previous block inherent data provider
//! so that the Tuxedo executive can scrape it to find the output references of the previous inherent transactions.

use parity_scale_codec::{Decode, Encode};
use scale_info::TypeInfo;
use serde::{Deserialize, Serialize};
use sp_core::H256;
use sp_inherents::{
    CheckInherentsResult, InherentData, InherentIdentifier, IsFatalError, MakeFatalError,
};
use sp_std::{vec, vec::Vec};

use crate::{types::Transaction, ConstraintChecker, SimpleConstraintChecker, Verifier};

/// An inherent identifier for the Tuxedo parent block inherent
pub const PARENT_INHERENT_IDENTIFIER: InherentIdentifier = *b"prnt_blk";

/// An inherent data provider that inserts the previous block into the inherent data.
/// This data does NOT go into an extrinsic.
#[cfg(feature = "std")]
pub struct ParentBlockInherentDataProvider<Block>(pub Block);

#[cfg(feature = "std")]
impl<B> sp_std::ops::Deref for ParentBlockInherentDataProvider<B> {
    type Target = B;

    fn deref(&self) -> &Self::Target {
        &self.0
    }
}
#[cfg(feature = "std")]
#[async_trait::async_trait]
impl<B: sp_runtime::traits::Block> sp_inherents::InherentDataProvider
    for ParentBlockInherentDataProvider<B>
{
    async fn provide_inherent_data(
        &self,
        inherent_data: &mut InherentData,
    ) -> Result<(), sp_inherents::Error> {
        inherent_data.put_data(PARENT_INHERENT_IDENTIFIER, &self.0)
    }

    async fn try_handle_error(
        &self,
        identifier: &InherentIdentifier,
        error: &[u8],
    ) -> Option<Result<(), sp_inherents::Error>> {
        if identifier == &PARENT_INHERENT_IDENTIFIER {
            println!("UH OH! INHERENT ERROR!!!!!!!!!!!!!!!!!!!!!!");
            Some(Err(sp_inherents::Error::Application(Box::from(
                <String as parity_scale_codec::Decode>::decode(&mut &error[..]).ok()?,
            ))))
        } else {
            None
        }
    }
}

/// Tuxedo's controlled interface around Substrate's concept of inherents.
///
/// This interface assumes that each inherent will appear exactly once in each block.
/// This will be verified off-chain by nodes before block execution begins.
///
/// This interface is stricter and more structured, and therefore simpler than FRAME's.
/// If you need to do something more powerful (which you probably don't) and you
/// understand exactly how Substrate's block authoring and Tuxedo's piece aggregation works
/// (which you probably don't) you can directly implement the `InherentInternal` trait
/// which is more powerful (and dangerous).
pub trait InherentHooks: SimpleConstraintChecker + Sized {
    type Error: Encode + IsFatalError;

    const INHERENT_IDENTIFIER: InherentIdentifier;

    /// Create the inherent extrinsic to insert into a block that is being authored locally.
    /// The inherent data is supplied by the authoring node.
    fn create_inherent<V: Verifier>(
        authoring_inherent_data: &InherentData,
        previous_inherent: (Transaction<V, Self>, H256),
    ) -> Transaction<V, Self>;

    /// Perform off-chain pre-execution checks on the inherent.
    /// The inherent data is supplied by the importing node.
    /// The inherent data available here is not guaranteed to be the
    /// same as what is available at authoring time.
    fn check_inherent<V>(
        importing_inherent_data: &InherentData,
        inherent: Transaction<V, Self>,
        results: &mut CheckInherentsResult,
    );

    /// Return the genesis transactions that are required for this inherent.
    fn genesis_transactions<V: Verifier>() -> Vec<Transaction<V, Self>> {
        Vec::new()
    }
}

/// An adapter type to declare, at the runtime level, that Tuxedo pieces provide custom inherent hooks.
///
/// This adapter type satisfies the executive's expectations by implementing both `ConstraintChecker`.
/// The inherent logic checks to be sure that exactly one inherent is present before plumbing through to
/// the underlying `TuxedoInherent` implementation.
///
/// This type should encode exactly like the inner type.
#[derive(
    Serialize, Deserialize, Eq, PartialEq, Debug, Decode, Default, Encode, TypeInfo, Clone, Copy,
)]
pub struct InherentAdapter<C>(C);

/// Helper to transform an entire transaction by wrapping the constraint checker.
fn wrap_transaction<V, C>(unwrapped: Transaction<V, C>) -> Transaction<V, InherentAdapter<C>> {
    Transaction {
        inputs: unwrapped.inputs,
        peeks: unwrapped.peeks,
        outputs: unwrapped.outputs,
        checker: InherentAdapter(unwrapped.checker),
    }
}

// I wonder if this should be a deref impl instead?
/// Helper to transform an entire transaction by unwrapping the constraint checker.
fn unwrap_transaction<V, C>(wrapped: Transaction<V, InherentAdapter<C>>) -> Transaction<V, C> {
    Transaction {
        inputs: wrapped.inputs,
        peeks: wrapped.peeks,
        outputs: wrapped.outputs,
        checker: wrapped.checker.0,
    }
}

impl<C: SimpleConstraintChecker + InherentHooks + 'static> ConstraintChecker
    for InherentAdapter<C>
{
    type Error = <C as SimpleConstraintChecker>::Error;

    fn check(
        &self,
        input_data: &[crate::dynamic_typing::DynamicallyTypedData],
        evicted_input_data: &[crate::dynamic_typing::DynamicallyTypedData],
        peek_data: &[crate::dynamic_typing::DynamicallyTypedData],
        output_data: &[crate::dynamic_typing::DynamicallyTypedData],
    ) -> Result<sp_runtime::transaction_validity::TransactionPriority, Self::Error> {
        SimpleConstraintChecker::check(
            &self.0,
            input_data,
            evicted_input_data,
            peek_data,
            output_data,
        )
    }

    fn is_inherent(&self) -> bool {
        true
    }

    fn create_inherents<V: Verifier>(
        authoring_inherent_data: &InherentData,
        previous_inherents: Vec<(Transaction<V, Self>, H256)>,
    ) -> Vec<Transaction<V, Self>> {
        if previous_inherents.len() > 1 {
            panic!("Authoring a leaf inherent constraint checker, but multiple previous inherents were supplied.")
        }

        let (previous_inherent, hash) = previous_inherents
            .first()
            .cloned()
            .expect("Previous inherent exists.");
        let current_inherent = wrap_transaction(<C as InherentHooks>::create_inherent(
            authoring_inherent_data,
            (unwrap_transaction(previous_inherent), hash),
        ));

        vec![current_inherent]
    }

    fn check_inherents<V: Clone>(
        importing_inherent_data: &InherentData,
        inherents: Vec<Transaction<V, Self>>,
        results: &mut CheckInherentsResult,
    ) {
        if inherents.is_empty() {
            results
                .put_error(
                    *b"12345678",
                    &MakeFatalError::from(
                        "Tuxedo inherent expected exactly one inherent extrinsic but found zero",
                    ),
                )
                .expect("Should be able to put an error.");
            return;
        } else if inherents.len() > 1 {
            results
                .put_error(*b"12345678", &MakeFatalError::from("Tuxedo inherent expected exactly one inherent extrinsic but found multiple"))
                .expect("Should be able to put an error.");
            return;
        }
        let inherent = inherents
            .first()
            .cloned()
            .expect("Previous inherent exists.");
        <C as InherentHooks>::check_inherent(
            importing_inherent_data,
            unwrap_transaction(inherent),
            results,
        )
    }

    fn genesis_transactions<V: Verifier>() -> Vec<Transaction<V, Self>> {
        <C as InherentHooks>::genesis_transactions()
            .into_iter()
            .map(|gtx| wrap_transaction(gtx))
            .collect()
    }
}
