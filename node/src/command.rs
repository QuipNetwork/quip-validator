use crate::{
    benchmarking::{inherent_benchmark_data, RemarkBuilder, TransferKeepAliveBuilder},
    chain_spec,
    cli::{Cli, Subcommand},
    service,
};
use frame_benchmarking_cli::{BenchmarkCmd, ExtrinsicFactory, SUBSTRATE_REFERENCE_HARDWARE};
use quip_protocol_runtime::{Block, EXISTENTIAL_DEPOSIT};
use quip_transaction_crypto::{account_id_from_public, HybridPair};
use sc_cli::SubstrateCli;
use sc_service::PartialComponents;
use sp_core::Pair as _;

impl SubstrateCli for Cli {
    fn impl_name() -> String {
        "Substrate Node".into()
    }

    fn impl_version() -> String {
        env!("SUBSTRATE_CLI_IMPL_VERSION").into()
    }

    fn description() -> String {
        env!("CARGO_PKG_DESCRIPTION").into()
    }

    fn author() -> String {
        env!("CARGO_PKG_AUTHORS").into()
    }

    fn support_url() -> String {
        "support.anonymous.an".into()
    }

    fn copyright_start_year() -> i32 {
        2017
    }

    fn load_spec(&self, id: &str) -> Result<Box<dyn sc_service::ChainSpec>, String> {
        let spec = match id {
            "dev" => chain_spec::development_chain_spec()?,
            "" | "local" => chain_spec::local_chain_spec()?,
            "local3" | "local-3" | "local_three_validator" => {
                chain_spec::local_three_validator_chain_spec()?
            }
            "quip-testnet" | "quip_testnet" | "testnet" => chain_spec::quip_testnet_chain_spec()?,
            path => chain_spec::ChainSpec::from_json_file(std::path::PathBuf::from(path))?,
        };

        chain_spec::ensure_chain_spec_id_compatibility(spec.id())?;
        Ok(Box::new(spec))
    }
}

fn ensure_cli_chain_id_compatibility(cli: &Cli) -> sc_cli::Result<()> {
    if cli.run.shared_params.is_dev() {
        chain_spec::ensure_local_chain_id_feature().map_err(sc_cli::Error::Input)?;
    }

    Ok(())
}

/// Parse and run command line arguments
pub fn run() -> sc_cli::Result<()> {
    let cli = Cli::from_args();
    ensure_cli_chain_id_compatibility(&cli)?;

    match &cli.subcommand {
        Some(Subcommand::Key(cmd)) => cmd.run(&cli),
        Some(Subcommand::InsertHybridKey(cmd)) => cmd.run(&cli),
        #[allow(deprecated)]
        Some(Subcommand::BuildSpec(cmd)) => {
            let runner = cli.create_runner(cmd)?;
            runner.sync_run(|config| cmd.run(config.chain_spec, config.network))
        }
        Some(Subcommand::CheckBlock(cmd)) => {
            let runner = cli.create_runner(cmd)?;
            runner.async_run(|config| {
                let PartialComponents {
                    client,
                    task_manager,
                    import_queue,
                    ..
                } = service::new_partial(&config)?;
                Ok((cmd.run(client, import_queue), task_manager))
            })
        }
        Some(Subcommand::ExportChainSpec(cmd)) => {
            let chain_spec = cli.load_spec(&cmd.chain)?;
            cmd.run(chain_spec)
        }
        Some(Subcommand::ExportBlocks(cmd)) => {
            let runner = cli.create_runner(cmd)?;
            runner.async_run(|config| {
                let PartialComponents {
                    client,
                    task_manager,
                    ..
                } = service::new_partial(&config)?;
                Ok((cmd.run(client, config.database), task_manager))
            })
        }
        Some(Subcommand::ExportState(cmd)) => {
            let runner = cli.create_runner(cmd)?;
            runner.async_run(|config| {
                let PartialComponents {
                    client,
                    task_manager,
                    ..
                } = service::new_partial(&config)?;
                Ok((cmd.run(client, config.chain_spec), task_manager))
            })
        }
        Some(Subcommand::ImportBlocks(cmd)) => {
            let runner = cli.create_runner(cmd)?;
            runner.async_run(|config| {
                let PartialComponents {
                    client,
                    task_manager,
                    import_queue,
                    ..
                } = service::new_partial(&config)?;
                Ok((cmd.run(client, import_queue), task_manager))
            })
        }
        Some(Subcommand::PurgeChain(cmd)) => {
            let runner = cli.create_runner(cmd)?;
            runner.sync_run(|config| cmd.run(config.database))
        }
        Some(Subcommand::Revert(cmd)) => {
            let runner = cli.create_runner(cmd)?;
            runner.async_run(|config| {
                let PartialComponents {
                    client,
                    task_manager,
                    backend,
                    ..
                } = service::new_partial(&config)?;
                let aux_revert = Box::new(|client, _, blocks| {
                    sc_consensus_grandpa::revert(client, blocks)?;
                    Ok(())
                });
                Ok((cmd.run(client, backend, Some(aux_revert)), task_manager))
            })
        }
        Some(Subcommand::Benchmark(cmd)) => {
            let runner = cli.create_runner(cmd)?;

            runner.sync_run(|config| {
                // This switch needs to be in the client, since the client decides
                // which sub-commands it wants to support.
                match cmd {
                    BenchmarkCmd::Pallet(cmd) => {
                        if !cfg!(feature = "runtime-benchmarks") {
                            return Err(
                                "Runtime benchmarking wasn't enabled when building the node. \
							You can enable it with `--features runtime-benchmarks`."
                                    .into(),
                            );
                        }

                        cmd.run_with_spec::<sp_runtime::traits::HashingFor<Block>, ()>(Some(
                            config.chain_spec,
                        ))
                    }
                    BenchmarkCmd::Block(cmd) => {
                        let PartialComponents { client, .. } = service::new_partial(&config)?;
                        cmd.run(client)
                    }
                    #[cfg(not(feature = "runtime-benchmarks"))]
                    BenchmarkCmd::Storage(_) => Err(
                        "Storage benchmarking can be enabled with `--features runtime-benchmarks`."
                            .into(),
                    ),
                    #[cfg(feature = "runtime-benchmarks")]
                    BenchmarkCmd::Storage(cmd) => {
                        let PartialComponents {
                            client, backend, ..
                        } = service::new_partial(&config)?;
                        let db = backend.expose_db();
                        let storage = backend.expose_storage();
                        let shared_cache = backend.expose_shared_trie_cache();

                        cmd.run(config, client, db, storage, shared_cache)
                    }
                    BenchmarkCmd::Overhead(cmd) => {
                        let PartialComponents { client, .. } = service::new_partial(&config)?;
                        let ext_builder = RemarkBuilder::new(client.clone());

                        cmd.run(
                            config.chain_spec.name().into(),
                            client,
                            inherent_benchmark_data()?,
                            Vec::new(),
                            &ext_builder,
                            false,
                        )
                    }
                    BenchmarkCmd::Extrinsic(cmd) => {
                        let PartialComponents { client, .. } = service::new_partial(&config)?;
                        // Register the *Remark* and *TKA* builders.
                        let alice = HybridPair::from_string("//Alice", None)
                            .map_err(|e| format!("invalid benchmark seed //Alice: {e:?}"))?;
                        let ext_factory = ExtrinsicFactory(vec![
                            Box::new(RemarkBuilder::new(client.clone())),
                            Box::new(TransferKeepAliveBuilder::new(
                                client.clone(),
                                account_id_from_public(&alice.public()),
                                EXISTENTIAL_DEPOSIT,
                            )),
                        ]);

                        cmd.run(client, inherent_benchmark_data()?, Vec::new(), &ext_factory)
                    }
                    BenchmarkCmd::Machine(cmd) => {
                        cmd.run(&config, SUBSTRATE_REFERENCE_HARDWARE.clone())
                    }
                }
            })
        }
        Some(Subcommand::ChainInfo(cmd)) => {
            let runner = cli.create_runner(cmd)?;
            runner.sync_run(|config| cmd.run::<Block>(&config))
        }
        None => {
            let runner = cli.create_runner(&cli.run)?;
            runner.run_node_until_exit(|config| async move {
                match config.network.network_backend {
					sc_network::config::NetworkBackendType::Libp2p => service::new_full::<
						sc_network::NetworkWorker<
							quip_protocol_runtime::opaque::Block,
							<quip_protocol_runtime::opaque::Block as sp_runtime::traits::Block>::Hash,
						>,
					>(config)
					.map_err(sc_cli::Error::Service),
					sc_network::config::NetworkBackendType::Litep2p =>
						service::new_full::<sc_network::Litep2pNetworkBackend>(config)
							.map_err(sc_cli::Error::Service),
				}
            })
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use clap::Parser as _;

    #[test]
    fn dev_flag_requires_matching_runtime_artifact() {
        let cli = Cli::try_parse_from(["quip-network-node", "--dev"])
            .expect("--dev should be valid CLI syntax");
        let result = ensure_cli_chain_id_compatibility(&cli);

        #[cfg(feature = "dev-chain-id")]
        assert!(result.is_ok());

        #[cfg(not(feature = "dev-chain-id"))]
        {
            let error = result.expect_err("default/testnet builds must reject --dev");
            assert!(error.to_string().contains("dev-chain-id"));
        }
    }

    #[test]
    fn local_chain_specs_require_matching_runtime_artifact() {
        let results = [
            chain_spec::development_chain_spec(),
            chain_spec::local_chain_spec(),
            chain_spec::local_three_validator_chain_spec(),
        ];

        for result in results {
            #[cfg(feature = "dev-chain-id")]
            assert!(result.is_ok());

            #[cfg(not(feature = "dev-chain-id"))]
            {
                let error = match result {
                    Ok(_) => panic!("default/testnet builds must reject local presets"),
                    Err(error) => error,
                };
                assert!(error.contains("dev-chain-id"));
            }
        }
    }

    #[test]
    fn testnet_chain_spec_requires_matching_runtime_artifact() {
        let result = chain_spec::quip_testnet_chain_spec();

        #[cfg(not(feature = "dev-chain-id"))]
        assert!(result.is_ok());

        #[cfg(feature = "dev-chain-id")]
        {
            let error = match result {
                Ok(_) => panic!("development builds must reject the public testnet preset"),
                Err(error) => error,
            };
            assert!(error.contains("20033"));
        }
    }

    #[test]
    fn chain_spec_ids_cannot_bypass_runtime_artifact_split() {
        let testnet = chain_spec::ensure_chain_spec_id_compatibility("quip_testnet");
        let local = chain_spec::ensure_chain_spec_id_compatibility("local_testnet");
        let custom = chain_spec::ensure_chain_spec_id_compatibility("custom_local");

        #[cfg(feature = "dev-chain-id")]
        {
            assert!(testnet.is_err());
            assert!(local.is_ok());
            assert!(custom.is_ok());
        }

        #[cfg(not(feature = "dev-chain-id"))]
        {
            assert!(testnet.is_ok());
            assert!(local.is_err());
            assert!(custom.is_err());
        }
    }
}
