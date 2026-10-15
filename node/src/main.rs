//! Substrate Node Template CLI library.
#![warn(missing_docs)]

mod benchmarking;
mod chain_spec;
mod cli;
mod command;
mod insert_hybrid_key;
mod rpc;
mod service;
mod work;

fn main() -> sc_cli::Result<()> {
    command::run()
}
