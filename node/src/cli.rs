#[derive(Debug, clap::Parser)]
pub struct Cli {
    #[command(subcommand)]
    pub subcommand: Option<Subcommand>,

    #[clap(flatten)]
    pub run: sc_cli::RunCmd,
}

#[derive(Debug, clap::Subcommand)]
#[allow(clippy::large_enum_variant)]
pub enum Subcommand {
    /// Key management cli utilities
    #[command(subcommand)]
    Key(sc_cli::KeySubcommand),

    /// Insert a hybrid post-quantum BABE or GRANDPA session key into the
    /// keystore. Derives the standard key-type id (`babe` / `gran`) from the
    /// chosen scheme; the equivalent stock command is
    /// `key insert --scheme hybrid-babe-h444|hybrid-grandpa-h244 --key-type ...`.
    ///
    /// If the node's keystore has a password, use the same --password,
    /// --password-filename or --password-interactive option you start the node
    /// with: the keystore re-derives the key with that password on every
    /// start. --password-filename is read verbatim, including a trailing
    /// newline.
    InsertHybridKey(crate::insert_hybrid_key::InsertHybridKeyCmd),

    /// Build a chain specification.
    /// DEPRECATED: `build-spec` command will be removed after 1/04/2026. Use `export-chain-spec`
    /// command instead.
    #[deprecated(
        note = "build-spec command will be removed after 1/04/2026. Use export-chain-spec command instead"
    )]
    BuildSpec(sc_cli::BuildSpecCmd),

    /// Export the chain specification.
    ExportChainSpec(sc_cli::ExportChainSpecCmd),

    /// Validate blocks.
    CheckBlock(sc_cli::CheckBlockCmd),

    /// Export blocks.
    ExportBlocks(sc_cli::ExportBlocksCmd),

    /// Export the state of a given block into a chain spec.
    ExportState(sc_cli::ExportStateCmd),

    /// Import blocks.
    ImportBlocks(sc_cli::ImportBlocksCmd),

    /// Remove the whole chain.
    PurgeChain(sc_cli::PurgeChainCmd),

    /// Revert the chain to a previous state.
    Revert(sc_cli::RevertCmd),

    /// Sub-commands concerned with benchmarking.
    #[command(subcommand)]
    Benchmark(frame_benchmarking_cli::BenchmarkCmd),

    /// Db meta columns information.
    ChainInfo(sc_cli::ChainInfoCmd),
}
