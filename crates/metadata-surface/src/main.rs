//! `metadata-surface [--types] OLD.scale NEW.scale` prints the pallet-level
//! surface diff between two V16 metadata blobs. `metadata-surface [--types]
//! FILE.scale` prints one surface. Exit status 1 means the surfaces differ.
use std::process::ExitCode;

fn load(path: &str, include_types: bool) -> Result<Vec<String>, String> {
    let bytes = std::fs::read(path).map_err(|error| format!("{path}: {error}"))?;
    let metadata = metadata_surface::decode(&bytes).map_err(|error| format!("{path}: {error}"))?;
    Ok(metadata_surface::surface(&metadata, include_types))
}

fn run() -> Result<bool, String> {
    let mut args: Vec<String> = std::env::args().skip(1).collect();
    let include_types = args.first().is_some_and(|arg| arg == "--types");
    if include_types {
        args.remove(0);
    }
    match args.as_slice() {
        [file] => {
            load(file, include_types)?
                .iter()
                .for_each(|line| println!("{line}"));
            Ok(false)
        }
        [old, new] => {
            let delta =
                metadata_surface::diff(&load(old, include_types)?, &load(new, include_types)?);
            delta.iter().for_each(|line| println!("{line}"));
            Ok(!delta.is_empty())
        }
        _ => Err("usage: metadata-surface [--types] OLD.scale [NEW.scale]".into()),
    }
}

fn main() -> ExitCode {
    match run() {
        Ok(false) => ExitCode::SUCCESS,
        Ok(true) => ExitCode::from(1),
        Err(error) => {
            eprintln!("{error}");
            ExitCode::from(2)
        }
    }
}
