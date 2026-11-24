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
use sp_core::crypto::{ExposeSecret, KeyTypeId, SecretStringError};
use sp_core::Pair;
use sp_keystore::KeystorePtr;
use std::str::FromStr;

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
    /// The secret key URI: use a bare BIP-39 mnemonic, in a file only you can
    /// read, or leave this out to be prompted for it.
    ///
    /// A bare mnemonic is its 12-24 words (BIP-39 allows 12, 15, 18, 21 or 24)
    /// separated by single spaces, with no quotes, derivation path (`//...`)
    /// or password (`///...`). Paths such as `//Alice` are for development
    /// chains.
    ///
    /// If the value names a file, the file's contents are used (a UTF-8
    /// byte-order mark and surrounding whitespace are dropped, as they are
    /// from a SURI typed at the prompt). If omitted, you are prompted for the
    /// SURI and the input is not shown. A SURI typed on
    /// the command line is visible in shell history and the process list.
    #[arg(long, value_name = "FILE")]
    pub suri: Option<String>,

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
        let suri = read_suri(self.suri.as_deref(), || {
            rpassword::prompt_password("Secret URI (input hidden): ")
        })?;

        let base_path = self
            .shared_params
            .base_path()?
            .unwrap_or_else(|| BasePath::from_project("", "", &C::executable_name()));
        let chain_id = self.shared_params.chain_id(self.shared_params.is_dev());
        let chain_spec = cli.load_spec(&chain_id)?;
        let config_dir = base_path.config_dir(chain_spec.id());

        let (keystore_path, password) = match self.keystore_params.keystore_config(&config_dir)? {
            KeystoreConfig::Path { path, password } => (path, password),
            _ => unreachable!("keystore_config always returns Path; qed"),
        };
        // Derive with the keystore's password, as `sc-cli key insert` does.
        // The keystore stores the SURI and re-derives the pair from it on
        // every load with `from_string(&suri, keystore_password)`
        // (`LocalKeystore::key_pair_by_type`). A key derived here without
        // that password would be filed under a public key the keystore then
        // fails to reproduce (`PublicKeyMismatch`), and the validator would
        // not author.
        let password_str = password.as_ref().map(|p| p.expose_secret().as_str());
        if password_str.is_some() && suri_has_password(&suri) {
            return Err(Error::Input(
                "the SURI has its own `///` password, and a keystore password is set: the \
                 keystore password replaces the SURI's when the key is inserted and loaded, so \
                 the stored key would not be the one this SURI gives elsewhere. Remove one of \
                 the two; the SURI is not shown"
                    .into(),
            ));
        }

        // `Public` implements both `AsRef<[u8]>` and `AsRef<InnerPublic>` since
        // upstream `d125cbde` (polkadot-sdk v0.2). Bind the public to a local
        // first, then disambiguate the byte-slice borrow explicitly.
        let (key_type, public_bytes) = match self.scheme {
            HybridScheme::HybridBabeH444 => {
                let pair = HybridBabePair::from_string(&suri, password_str)
                    .map_err(|e| invalid_suri("hybrid-babe-h444", &e))?;
                let key_type = self.resolve_key_type(sp_consensus_babe::KEY_TYPE)?;
                let public = pair.public();
                let bytes: &[u8] = public.as_ref();
                (key_type, bytes.to_vec())
            }
            HybridScheme::HybridGrandpaH244 => {
                let pair = HybridGrandpaPair::from_string(&suri, password_str)
                    .map_err(|e| invalid_suri("hybrid-grandpa-h244", &e))?;
                let key_type = self.resolve_key_type(sp_consensus_grandpa::KEY_TYPE)?;
                let public = pair.public();
                let bytes: &[u8] = public.as_ref();
                (key_type, bytes.to_vec())
            }
        };

        let keystore: KeystorePtr = LocalKeystore::open(keystore_path, password)?.into();

        // `Keystore::insert` returns `Result<(), ()>`, so there is nothing to
        // show. The `()` pattern keeps it that way: if the error ever gains a
        // payload (which could hold the SURI), this stops compiling instead of
        // printing it.
        keystore
            .insert(key_type, &suri, &public_bytes)
            .map_err(|()| {
                Error::Input(format!("keystore insert failed for key_type={key_type:?}"))
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

/// Whether `suri` carries an explicit `///` password, even an empty one.
///
/// Parses with sp-core's own SURI grammar. A SURI that does not parse has no
/// password to conflict with; it is rejected by the derivation instead, with
/// an error that does not show it.
fn suri_has_password(suri: &str) -> bool {
    sp_core::crypto::SecretUri::from_str(suri).is_ok_and(|uri| uri.password.is_some())
}

/// The SURI to insert.
///
/// - No `--suri`: ask `prompt`, which reads it from the terminal without
///   showing it, so the secret stays out of shell history and the process
///   list.
/// - `--suri` naming a readable file: its contents without a leading UTF-8
///   byte-order mark, trimmed. Editors on Windows save "UTF-8" files with a
///   BOM, which `trim` keeps (U+FEFF is not whitespace) and sp-core rejects
///   as an invalid character.
/// - Otherwise: the `--suri` value as given.
///
/// Mirrors the file-or-literal-or-prompt handling in
/// `sc_cli::commands::utils::read_uri`.
fn read_suri(
    suri: Option<&str>,
    prompt: impl FnOnce() -> std::io::Result<String>,
) -> sc_cli::Result<String> {
    let Some(suri) = suri else {
        let typed = prompt().map_err(|e| {
            Error::Input(format!(
                "no --suri given, and the hidden prompt failed ({e}); pass --suri <FILE>"
            ))
        })?;
        return Ok(clean_suri_text(&typed));
    };
    let path = std::path::Path::new(suri);
    if path.is_file() {
        let raw = std::fs::read_to_string(path)
            .map_err(|e| Error::Input(format!("failed to read SURI file {suri}: {e}")))?;
        Ok(clean_suri_text(&raw))
    } else {
        Ok(suri.to_owned())
    }
}

/// The SURI in text an operator saved or pasted: without a leading UTF-8
/// byte-order mark, and trimmed (`trim` also drops a trailing tab or a
/// no-break space). The file and the prompt get the same treatment, so a
/// pasted SURI loads exactly as the same SURI saved to a file does.
fn clean_suri_text(raw: &str) -> String {
    raw.strip_prefix('\u{feff}')
        .unwrap_or(raw)
        .trim()
        .to_owned()
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

    #[test]
    fn suri_is_optional_on_the_command_line() {
        let cmd = InsertHybridKeyCmd::try_parse_from([
            "insert-hybrid-key",
            "--scheme",
            "hybrid-babe-h444",
        ])
        .expect("--suri may be omitted");
        assert_eq!(cmd.suri, None);
    }

    #[test]
    fn suri_help_recommends_a_bare_mnemonic_and_the_prompt() {
        use clap::CommandFactory;
        let cmd = InsertHybridKeyCmd::command();
        let suri = cmd
            .get_arguments()
            .find(|arg| arg.get_id() == "suri")
            .expect("--suri");
        let short = suri.get_help().expect("short help").to_string();
        assert!(short.contains("bare BIP-39 mnemonic"), "{short}");
        assert!(short.contains("prompted"), "{short}");
        let long = suri.get_long_help().expect("long help").to_string();
        assert!(long.contains("shell history"), "{long}");
        assert!(long.contains("12-24 words"), "{long}");
        assert!(!long.contains("12 or 24"), "{long}");
    }

    #[test]
    fn without_suri_the_prompt_supplies_it() {
        let mut asked = 0;
        let suri = read_suri(None, || {
            asked += 1;
            Ok(PHRASE.to_owned())
        })
        .expect("prompted SURI");
        assert_eq!(suri, PHRASE);
        assert_eq!(asked, 1);
    }

    /// A pasted SURI is cleaned as a file's contents are: a BOM and
    /// surrounding whitespace (tab, newline, no-break space) are dropped.
    #[test]
    fn a_pasted_suri_is_cleaned_like_a_file() {
        for typed in [
            format!("\u{feff}{PHRASE}"),
            format!("{PHRASE}\t"),
            format!("{PHRASE}\u{a0}"),
            format!("  {PHRASE}\r\n"),
        ] {
            let suri = read_suri(None, || Ok(typed.clone())).expect("prompted SURI");
            assert_eq!(suri, PHRASE, "{typed:?}");
        }
    }

    /// An explicit `///` password, even an empty one, is detected; a SURI
    /// without one, or one that does not parse, is not.
    #[test]
    fn an_explicit_suri_password_is_detected() {
        assert!(suri_has_password(&format!("{PHRASE}///pw")));
        assert!(suri_has_password(&format!("{PHRASE}///")));
        assert!(suri_has_password("//Alice///pw"));
        assert!(!suri_has_password(PHRASE));
        assert!(!suri_has_password("//Alice"));
        assert!(!suri_has_password(&format!("\"{PHRASE}\"///pw")));
    }

    #[test]
    fn with_suri_the_prompt_is_not_used() {
        let suri = read_suri(Some("//Alice"), || panic!("must not prompt")).expect("literal");
        assert_eq!(suri, "//Alice");
    }

    #[test]
    fn a_failed_prompt_says_to_pass_suri() {
        let Err(e) = read_suri(None, || Err(std::io::Error::other("no terminal"))) else {
            panic!("a failed prompt is an error");
        };
        let text = e.to_string();
        assert!(
            text.contains("--suri") && text.contains("no terminal"),
            "{text}"
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
