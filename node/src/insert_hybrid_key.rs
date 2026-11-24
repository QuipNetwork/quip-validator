//! `insert-hybrid-key` subcommand: insert a hybrid post-quantum BABE or
//! GRANDPA session key into the validator's keystore.
//!
//! Stock `sc-cli` does also accept these schemes via `key insert --scheme
//! hybrid-babe-h444` / `--scheme hybrid-grandpa-h244` (see the sibling
//! patch in `QuipNetwork/polkadot-sdk@v0.2`), but this subcommand exists as
//! a more focused entry point:
//!
//! * it derives the correct [`KeyTypeId`] automatically from the scheme
//!   (BABE → `babe`, GRANDPA → `gran`), removing the easy footgun of inserting
//!   a hybrid key under the wrong key-type id;
//! * it pulls the hybrid `Pair` types directly from `quip-crypto-primitives`,
//!   so the code path is independent of any future drift in `sc_cli`.
//!
//! Both entry points write to the same on-disk format and are interchangeable
//! for runtime consumption.

use clap::{Parser, ValueEnum};
use quip_crypto_primitives::substrate::{
    ed25519_fndsa512::Pair as HybridGrandpaPair, sr25519_fndsa512::Pair as HybridBabePair,
};
use sc_cli::{Error, KeystoreParams, SharedParams, SubstrateCli};
use sc_keystore::LocalKeystore;
use sc_service::config::{BasePath, KeystoreConfig};
use sp_core::crypto::{KeyTypeId, SecretStringError};
use sp_core::Pair;
use sp_keystore::KeystorePtr;

/// Hybrid post-quantum scheme to insert.
#[derive(Debug, Copy, Clone, PartialEq, Eq, ValueEnum)]
#[value(rename_all = "kebab-case")]
pub enum HybridScheme {
    /// Hybrid `sr25519 + FN-DSA-512` (H444) — BABE consensus.
    HybridBabeH444,
    /// Hybrid `ed25519 + FN-DSA-512` (H244) — GRANDPA finality.
    HybridGrandpaH244,
}

/// The `insert-hybrid-key` command.
#[derive(Debug, Clone, Parser)]
#[command(
    name = "insert-hybrid-key",
    about = "Insert a hybrid (post-quantum) BABE or GRANDPA session key into the keystore."
)]
pub struct InsertHybridKeyCmd {
    /// The secret key URI (BIP39 mnemonic or derivation path like `//Alice`).
    /// If the value names a file, the file's contents are used instead.
    #[arg(long)]
    pub suri: String,

    /// Override the runtime key-type id. Defaults to the standard id for the
    /// chosen scheme (`babe` for HybridBabeH444, `gran` for HybridGrandpaH244).
    #[arg(long, value_name = "ID")]
    pub key_type: Option<String>,

    /// Which hybrid post-quantum scheme to insert.
    #[arg(long, value_name = "SCHEME", value_enum, ignore_case = true)]
    pub scheme: HybridScheme,

    #[allow(missing_docs)]
    #[clap(flatten)]
    pub shared_params: SharedParams,

    #[allow(missing_docs)]
    #[clap(flatten)]
    pub keystore_params: KeystoreParams,
}

impl InsertHybridKeyCmd {
    /// Run the command.
    pub fn run<C: SubstrateCli>(&self, cli: &C) -> sc_cli::Result<()> {
        let suri = read_suri(&self.suri)?;

        let base_path = self
            .shared_params
            .base_path()?
            .unwrap_or_else(|| BasePath::from_project("", "", &C::executable_name()));
        let chain_id = self.shared_params.chain_id(self.shared_params.is_dev());
        let chain_spec = cli.load_spec(&chain_id)?;
        let config_dir = base_path.config_dir(chain_spec.id());

        // `Public` implements both `AsRef<[u8]>` and `AsRef<InnerPublic>` since
        // upstream `d125cbde` (polkadot-sdk v0.2). Bind the public to a local
        // first, then disambiguate the byte-slice borrow explicitly.
        let (key_type, public_bytes) = match self.scheme {
            HybridScheme::HybridBabeH444 => {
                let pair = HybridBabePair::from_string(&suri, None)
                    .map_err(|e| invalid_suri("hybrid-babe-h444", &e))?;
                let key_type = self.resolve_key_type(sp_consensus_babe::KEY_TYPE)?;
                let public = pair.public();
                let bytes: &[u8] = public.as_ref();
                (key_type, bytes.to_vec())
            }
            HybridScheme::HybridGrandpaH244 => {
                let pair = HybridGrandpaPair::from_string(&suri, None)
                    .map_err(|e| invalid_suri("hybrid-grandpa-h244", &e))?;
                let key_type = self.resolve_key_type(sp_consensus_grandpa::KEY_TYPE)?;
                let public = pair.public();
                let bytes: &[u8] = public.as_ref();
                (key_type, bytes.to_vec())
            }
        };

        let keystore: KeystorePtr = match self.keystore_params.keystore_config(&config_dir)? {
            KeystoreConfig::Path { path, password } => LocalKeystore::open(path, password)?.into(),
            _ => unreachable!("keystore_config always returns Path; qed"),
        };

        keystore
            .insert(key_type, &suri, &public_bytes)
            .map_err(|e| {
                Error::Input(format!(
                    "keystore insert failed for key_type={key_type:?}: {e:?}"
                ))
            })?;

        Ok(())
    }

    fn resolve_key_type(&self, default: KeyTypeId) -> sc_cli::Result<KeyTypeId> {
        match &self.key_type {
            Some(s) => KeyTypeId::try_from(s.as_str()).map_err(|_| {
                Error::Input(format!("--key-type {s:?} is not a valid 4-byte KeyTypeId"))
            }),
            None => Ok(default),
        }
    }
}

/// The error for a SURI that does not parse.
///
/// It names the kind of problem in fixed words and never shows the SURI:
/// `SecretStringError::InvalidFormat` carries a full copy of the input
/// (`InvalidCharacterInfo`), so its `Debug` and `Display` print the
/// mnemonic and the password. Only the variant is used here.
fn invalid_suri(scheme: &str, e: &SecretStringError) -> Error {
    let reason = match e {
        SecretStringError::InvalidFormat(_) => {
            "it contains a character that is not allowed (look for quotes, \
             a byte-order mark or a line break)"
        }
        SecretStringError::InvalidPhrase => "it is not a valid BIP-39 mnemonic",
        SecretStringError::InvalidPassword => "the password is not valid",
        SecretStringError::InvalidSeed => "the seed is not valid",
        SecretStringError::InvalidSeedLength => "the seed has the wrong length",
        SecretStringError::InvalidPath => "the derivation path is not valid",
    };
    Error::Input(format!(
        "invalid SURI for {scheme}: {reason}; the SURI is not shown"
    ))
}

/// If `suri` names a readable file, return its trimmed contents; otherwise
/// return `suri` as-is.
///
/// Mirrors the file-or-literal handling in `sc_cli::commands::utils::read_uri`
/// without depending on its non-prompted code path.
fn read_suri(suri: &str) -> sc_cli::Result<String> {
    let path = std::path::Path::new(suri);
    if path.is_file() {
        let raw = std::fs::read_to_string(path)
            .map_err(|e| Error::Input(format!("failed to read SURI file {suri}: {e}")))?;
        Ok(raw.trim().to_owned())
    } else {
        Ok(suri.to_owned())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hybrid_scheme_cli_names_match_sdk_key_schemes() {
        assert_eq!(
            <HybridScheme as ValueEnum>::from_str("hybrid-babe-h444", false),
            Ok(HybridScheme::HybridBabeH444),
        );
        assert_eq!(
            <HybridScheme as ValueEnum>::from_str("hybrid-grandpa-h244", false),
            Ok(HybridScheme::HybridGrandpaH244),
        );
    }

    /// A published BIP-39 test vector, standing in for a secret.
    const PHRASE: &str =
        "legal winner thank year wave sausage worth useful legal winner thank yellow";

    /// The echo came from the SDK error itself: `InvalidFormat` holds a copy
    /// of the SURI. `invalid_suri` keeps only the variant.
    #[test]
    fn the_invalid_suri_error_names_the_problem_without_the_suri() {
        let quoted = format!("\"{PHRASE}///hunter2\"");
        let Err(e) = HybridBabePair::from_string(&quoted, None) else {
            panic!("a quoted SURI must not parse");
        };
        assert!(matches!(e, SecretStringError::InvalidFormat(_)));
        assert!(
            format!("{e:?}").contains("sausage"),
            "the SDK error carries the SURI"
        );

        let shown = invalid_suri("hybrid-babe-h444", &e);
        for text in [format!("{shown}"), format!("{shown:?}")] {
            assert!(text.contains("invalid SURI for hybrid-babe-h444"), "{text}");
            for secret in PHRASE.split(' ').chain(["hunter2"]) {
                assert!(!text.contains(secret), "{text} shows {secret:?}");
            }
        }
    }
}
