//! A simple CLI wallet. For now it is a toy just to start testing things out.

use std::path::PathBuf;

use anyhow::anyhow;
use clap::{ArgAction::Append, Args, Parser, Subcommand};
use jsonrpsee::{
    core::client::ClientT,
    http_client::{HttpClient, HttpClientBuilder},
    rpc_params,
};
use parity_scale_codec::{Decode, Encode};
use sp_keystore::SyncCryptoStore;
use sp_runtime::{CryptoTypeId, KeyTypeId};
use tuxedo_core::{
    types::{Output, OutputRef},
    Verifier,
};

use sp_core::{
    crypto::{CryptoTypePublicPair, Pair as PairT},
    sr25519::Pair,
    H256,
};

mod amoeba;
mod money;

/// The default RPC endpoint for the wallet to connect to
const DEFAULT_ENDPOINT: &str = "http://localhost:9933";

// TODO Get an OS appropriate path like Substrate does using `directories` crate
// https://docs.rs/directories/latest/directories/struct.ProjectDirs.html
// https://github.com/paritytech/substrate/blob/master/client/service/src/config.rs#L319
// https://github.com/paritytech/substrate/blob/master/client/cli/src/config.rs#L489

// https://doc.rust-lang.org/std/env/fn.home_dir.html exists but is deprecated.
/// The default path for the keystore that stores the keys for signing transactions
const DEFAULT_DATA_PATH: &str = "~/.local/share/tuxedo-template-wallet";

/// A KeyTypeId to use in the keystore for Tuxedo transactions. We'll use this everywhere
/// until it becomes clear that there si a reason to use multiple of them
const KEY_TYPE: KeyTypeId = KeyTypeId(*b"_tux");

/// A default seed phrase for signing inputs when none is provided
/// Corresponds to the default pubkey.
const SHAWN_PHRASE: &str =
    "news slush supreme milk chapter athlete soap sausage put clutch what kitten";

const SHAWN_PUB_KEY: &str = "d2bf4b844dfefd6772a8843e669f943408966a977e3ae2af1dd78e0f55f4df67";

/// The wallet's main CLI struct
#[derive(Debug, Parser)]
#[command(about, version)]
struct Cli {
    #[arg(long, short, default_value_t = DEFAULT_ENDPOINT.to_string())]
    /// RPC endpoint of the node that this wallet will connect to
    endpoint: String,

    #[arg(long, short, default_value_t = DEFAULT_DATA_PATH.to_string())]
    /// Path where to the wallet data is stored. Wallet data is just keystore at the moment,
    /// but will contain a local database of UTXOs in the future.
    data_path: String,

    #[command(subcommand)]
    command: Command,
}

/// The tasks supported by the wallet
#[derive(Debug, Subcommand)]
enum Command {
    /// Demonstrate creating an amoeba and performing mitosis on it.
    AmoebaDemo,

    /// Verify that a particular coin exists in storage. Show its value and owner.
    VerifyCoin {
        /// A hex-encoded output reference
        #[arg(value_parser = output_ref_from_string)]
        output_ref: OutputRef,
    },

    /// Spend some coins. For now, all outputs go to the same recipient.
    SpendCoins(SpendArgs),

    /// Insert a private key into the keystore to later use when signing transactions.
    InsertKey {
        /// Seed phrase of the key to insert.
        seed: String,
    },

    /// Show public information about all the keys in the keystore.
    ShowKeys,

    /// Remove a specific key from the keystore.
    /// WARNING! This will permanently delete the private key information. Make sure your
    /// keys are backed up somewhere safe.
    RemoveKey {
        /// The public key to remove
        #[arg(value_parser = pubkey_h256_from_string)]
        pub_key: H256,
    },
}

#[derive(Debug, Args)]
pub struct SpendArgs {
    /// An input to be consumed by this transaction. This argument may be specified multiple times.
    /// They must all be coins.
    #[arg(long, short, value_parser = output_ref_from_string)]
    input: Vec<OutputRef>,

    // Jesus m-fkin christ. It took me a literal hour to dig through docs and github to find
    // https://docs.rs/clap/latest/clap/_derive/_cookbook/typed_derive/index.html which finally showed
    // how to specify a custom parsing function
    /// Hex encoded address (sr25519 pubkey) of the recipient
    #[arg(long, short, value_parser = pubkey_h256_from_string, default_value = SHAWN_PUB_KEY)]
    recipient: H256,

    // The `action = Append` allows us to accept the same value multiple times.
    /// An output amount. For the transaction to be valid, the outputs must add up to less than
    /// the sum of the inputs. The wallet will not enforce this and will gladly send an invalid transaction
    /// which will then e rejected by the node.
    #[arg(long, short, action = Append)]
    output_amount: Vec<u128>,
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    // Parse command line args
    let cli = Cli::parse();

    // Setup the keystore.
    let data_path = PathBuf::from(cli.data_path);
    let keystore_path = data_path.join("keystore");
    let keystore = sc_keystore::LocalKeystore::open(keystore_path, None)?;

    // If the keystore is empty, insert the example Shawn key so example transactions can be signed.
    if keystore.keys(KEY_TYPE)?.is_empty() {
        // This only inserts it into memory. That should be fine for the example key since it can always be
        // re-inserted on each new run. But for user-provided keys, we want them to be persisted.
        // Hopefully insert_unknown will make that happen.
        keystore
            .sr25519_generate_new(KEY_TYPE, Some(SHAWN_PHRASE))
            .map_err(|e| anyhow!("{:?}", e))?;
    }

    // Setup jsonrpsee and endpoint-related information.
    // https://github.com/paritytech/jsonrpsee/blob/master/examples/examples/http.rs
    let client = HttpClientBuilder::default().build(cli.endpoint)?;

    // Dispatch to proper subcommand
    match cli.command {
        Command::AmoebaDemo => amoeba::amoeba_demo(&client).await,
        Command::VerifyCoin { output_ref } => {
            money::print_coin_from_storage(&output_ref, &client).await
        }
        Command::SpendCoins(args) => money::spend_coins(&client, &keystore, args).await,
        Command::InsertKey { seed } => {
            // We need to provide a public key to the keystore manually, so let's calculate it.
            let public_key = Pair::from_phrase(&seed, None)?.0.public();
            println!("The generated public key is {:?}", public_key);
            keystore
                .insert_unknown(KEY_TYPE, &seed, public_key.as_ref())
                .map_err(|e| anyhow!("{:?}", e))?;
            Ok(())
        }
        Command::ShowKeys => {
            keystore
                .keys(KEY_TYPE)?
                .into_iter()
                .filter_map(|CryptoTypePublicPair(t, public)| {
                    // Since we insert with `insert_unknown`, each key is inserted three times.
                    // Here we filter out just the sr25519 variant so we don't print duplicates.
                    if t == CryptoTypeId(*b"sr25") {
                        Some(public)
                    } else {
                        None
                    }
                })
                .for_each(|pubkey| {
                    println!("key: 0x{}", hex::encode(pubkey));
                });

            Ok(())
        }
        Command::RemoveKey { pub_key } => {
            // The keystore doesn't provide an API for removing keys, so we
            // remove them from the filesystem directly
            let filename = format!("0x{}{}", hex::encode(KEY_TYPE.0), hex::encode(pub_key.0));

            println!("The filename is {}", filename);
            todo!()
        }
    }
}

/// Fetch an output from chain storage given an OutputRef
pub async fn fetch_storage<V: Verifier>(
    output_ref: &OutputRef,
    client: &HttpClient,
) -> anyhow::Result<Output<V>> {
    let ref_hex = hex::encode(output_ref.encode());
    let params = rpc_params![ref_hex];
    let rpc_response: Result<Option<String>, _> = client.request("state_getStorage", params).await;

    let response_hex = rpc_response?.ok_or(anyhow!("Data cannot be retrieved from storage"))?;
    let response_hex = strip_0x_prefix(&response_hex);
    let response_bytes = hex::decode(response_hex)?;
    let utxo = Output::decode(&mut &response_bytes[..])?;

    Ok(utxo)
}

/// Parse a string into an H256 that represents a public key
fn pubkey_h256_from_string(s: &str) -> Result<H256, clap::Error> {
    let s = strip_0x_prefix(s);

    let mut pubkey_bytes: [u8; 32] = [0; 32];
    hex::decode_to_slice(s, &mut pubkey_bytes as &mut [u8])
        .map_err(|_| clap::Error::new(clap::error::ErrorKind::ValueValidation))?;
    Ok(H256::from(pubkey_bytes))
}

/// Parse an output ref from a string
fn output_ref_from_string(s: &str) -> Result<OutputRef, clap::Error> {
    let s = strip_0x_prefix(s);
    let bytes =
        hex::decode(s).map_err(|_| clap::Error::new(clap::error::ErrorKind::ValueValidation))?;

    OutputRef::decode(&mut &bytes[..])
        .map_err(|_| clap::Error::new(clap::error::ErrorKind::ValueValidation))
}

/// Takes a string and checks for a 0x prefix. Returns a string without a 0x prefix.
fn strip_0x_prefix(s: &str) -> &str {
    if &s[..2] == "0x" {
        &s[2..]
    } else {
        s
    }
}
