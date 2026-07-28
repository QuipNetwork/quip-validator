#[path = "../tests/support/signing_fixture.rs"]
mod signing_fixture_support;

use std::{env, fs, path::PathBuf};

fn main() {
    let fixture = signing_fixture_support::generate_fixture();
    let rendered = format!(
        "{}\n",
        serde_json::to_string_pretty(&fixture).expect("fixture serializes")
    );

    if env::args().any(|arg| arg == "--write") {
        let path = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("../docs/polkadotjs/fixtures/hybrid-signing.json");
        fs::create_dir_all(path.parent().expect("fixture has a parent"))
            .expect("fixture directory can be created");
        fs::write(&path, rendered).expect("fixture can be written");
        println!("wrote {}", path.display());
    } else {
        print!("{rendered}");
    }
}
