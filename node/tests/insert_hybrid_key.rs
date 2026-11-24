//! `insert-hybrid-key` never echoes the secret URI (GregoAI wp-2 G-001).
//!
//! These tests drive the real `quip-network-node` binary and read what an
//! operator would see on the terminal: stdout and stderr of a failed insert.
//! A SURI that `sp-core` rejects for its *format* (a byte-order mark, a quote,
//! a line break) comes back as `SecretStringError::InvalidFormat`, whose
//! `InvalidCharacterInfo` holds a full copy of the input. Printing that error
//! printed the mnemonic and the password.

use std::path::{Path, PathBuf};
use std::process::{Command, Output};
use std::sync::atomic::{AtomicUsize, Ordering};

/// A valid BIP-39 test mnemonic (the `7f7f…` vector of the reference
/// implementation). It is a published test vector, not a real secret, but the
/// tests treat it as one: no word of it may reach the terminal.
const PHRASE: &str = "legal winner thank year wave sausage worth useful legal winner thank yellow";
/// A password that is just as secret as the phrase.
const PASSWORD: &str = "Tr0ub4dor-hunter2";

const SCHEMES: [&str; 2] = ["hybrid-babe-h444", "hybrid-grandpa-h244"];

/// A fresh, empty directory under the system temp dir, removed on drop so
/// no SURI file or keystore is left behind.
struct ScratchDir(PathBuf);

impl ScratchDir {
    fn join(&self, name: impl AsRef<Path>) -> PathBuf {
        self.0.join(name)
    }
}

impl Drop for ScratchDir {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

fn scratch_dir() -> ScratchDir {
    static NEXT: AtomicUsize = AtomicUsize::new(0);
    let dir = std::env::temp_dir().join(format!(
        "quip-insert-hybrid-key-{}-{}",
        std::process::id(),
        NEXT.fetch_add(1, Ordering::Relaxed)
    ));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).expect("create scratch dir");
    ScratchDir(dir)
}

fn insert(scheme: &str, suri: &str, base_path: &Path) -> Output {
    Command::new(env!("CARGO_BIN_EXE_quip-network-node"))
        .args([
            "insert-hybrid-key",
            "--dev",
            "--scheme",
            scheme,
            "--suri",
            suri,
        ])
        .arg("--base-path")
        .arg(base_path)
        .output()
        .expect("run quip-network-node")
}

/// The secret fragments the output shows: each phrase word and the
/// password, matched case-insensitively so an escaped or re-cased copy also
/// counts.
fn echoed(out: &Output) -> Vec<&'static str> {
    let shown = format!(
        "{}\n{}",
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr)
    )
    .to_lowercase();
    PHRASE
        .split(' ')
        .chain([PASSWORD])
        .filter(|secret| shown.contains(&secret.to_lowercase()))
        .collect()
}

/// What is wrong with a rejected insert's output, if anything.
fn rejection_problems(what: &str, out: &Output) -> Vec<String> {
    let mut problems = Vec::new();
    if out.status.success() {
        problems.push(format!("{what}: the insert should have failed"));
    }
    let stderr = String::from_utf8_lossy(&out.stderr);
    if !stderr.contains("invalid SURI") {
        problems.push(format!("{what}: stderr does not say the SURI is invalid"));
    }
    let shown = echoed(out);
    if !shown.is_empty() {
        problems.push(format!(
            "{what}: the output shows the secret fragments {shown:?}"
        ));
    }
    problems
}

/// Inputs that `sp-core` rejects for their format, each given both as a file
/// (the recommended form) and, where a shell could pass it, as a literal.
fn malformed_files() -> Vec<(&'static str, String)> {
    vec![
        ("a double-quoted phrase", format!("\"{PHRASE}\"\n")),
        ("a single-quoted phrase", format!("'{PHRASE}'\n")),
        (
            "one word per line",
            format!("{}\n", PHRASE.replace(' ', "\n")),
        ),
        (
            "a password with a line break",
            format!("{PHRASE}///{PASSWORD}\nx\n"),
        ),
    ]
}

#[test]
fn a_malformed_suri_file_is_rejected_without_echo() {
    let dir = scratch_dir();
    let mut problems = Vec::new();
    for scheme in SCHEMES {
        for (i, (what, contents)) in malformed_files().into_iter().enumerate() {
            let file = dir.join(format!("suri-{scheme}-{i}"));
            std::fs::write(&file, contents).expect("write SURI file");
            let what = format!("{scheme}, file with {what}");
            let out = insert(scheme, file.to_str().unwrap(), &dir.join("base"));
            problems.extend(rejection_problems(&what, &out));
        }
    }
    assert!(problems.is_empty(), "{}", problems.join("\n"));
}

#[test]
fn a_malformed_literal_suri_is_rejected_without_echo() {
    let dir = scratch_dir();
    let mut problems = Vec::new();
    for scheme in SCHEMES {
        for (what, literal) in [
            ("a trailing newline", format!("{PHRASE}\n")),
            ("a trailing carriage return", format!("{PHRASE}\r")),
            ("a double-quoted phrase", format!("\"{PHRASE}\"")),
        ] {
            let what = format!("{scheme}, literal with {what}");
            problems.extend(rejection_problems(
                &what,
                &insert(scheme, &literal, &dir.join("base")),
            ));
        }
    }
    assert!(problems.is_empty(), "{}", problems.join("\n"));
}

/// The positive control: the same harness inserts a well-formed SURI, so the
/// rejections above are not an artefact of how the binary is driven.
#[test]
fn a_well_formed_suri_file_is_inserted() {
    let dir = scratch_dir();
    for scheme in SCHEMES {
        let file = dir.join(format!("suri-{scheme}"));
        std::fs::write(&file, format!("{PHRASE}\n")).expect("write SURI file");
        let out = insert(scheme, file.to_str().unwrap(), &dir.join("base"));
        assert!(
            out.status.success(),
            "{scheme}: insert failed: {}",
            String::from_utf8_lossy(&out.stderr)
        );
        assert!(echoed(&out).is_empty(), "{scheme}: {:?}", echoed(&out));
    }
}

/// The keystore file names under `base`: `<key type hex><public key hex>`.
fn keystore_entries(base: &Path) -> Vec<String> {
    let keystore = base.join("chains").join("dev").join("keystore");
    let mut names: Vec<String> = std::fs::read_dir(&keystore)
        .unwrap_or_else(|e| panic!("read {}: {e}", keystore.display()))
        .map(|entry| {
            entry
                .expect("keystore entry")
                .file_name()
                .to_string_lossy()
                .into_owned()
        })
        .collect();
    names.sort();
    names
}

/// A SURI file saved with a UTF-8 byte-order mark (Windows editors do this)
/// inserts the same key as the plain file.
#[test]
fn a_suri_file_with_a_byte_order_mark_inserts_the_same_key() {
    let dir = scratch_dir();
    for scheme in SCHEMES {
        let plain = dir.join(format!("plain-{scheme}"));
        let bom = dir.join(format!("bom-{scheme}"));
        std::fs::write(&plain, format!("{PHRASE}\n")).expect("write SURI file");
        std::fs::write(&bom, format!("\u{feff}{PHRASE}\r\n")).expect("write SURI file");

        let plain_base = dir.join(format!("plain-base-{scheme}"));
        let bom_base = dir.join(format!("bom-base-{scheme}"));
        for (file, base) in [(&plain, &plain_base), (&bom, &bom_base)] {
            let out = insert(scheme, file.to_str().unwrap(), base);
            assert!(
                out.status.success(),
                "{scheme}: insert of {} failed: {}",
                file.display(),
                String::from_utf8_lossy(&out.stderr)
            );
            assert!(echoed(&out).is_empty(), "{scheme}: {:?}", echoed(&out));
        }
        let plain_keys = keystore_entries(&plain_base);
        assert_eq!(plain_keys.len(), 1, "{scheme}: {plain_keys:?}");
        assert_eq!(keystore_entries(&bom_base), plain_keys, "{scheme}");
    }
}
