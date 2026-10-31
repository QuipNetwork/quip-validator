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
            "local" => chain_spec::local_chain_spec()?,
            "rehearsal" | "validator_rehearsal" => chain_spec::validator_rehearsal_chain_spec()?,
            "local3" | "local-3" | "local_three_validator" => {
                chain_spec::local_three_validator_chain_spec()?
            }
            "" | "quip-testnet" | "quip_testnet" | "testnet" => {
                chain_spec::quip_testnet_chain_spec()?
            }
            path => chain_spec::ChainSpec::from_json_file(std::path::PathBuf::from(path))?,
        };

        Ok(Box::new(spec))
    }
}

/// Parse and run command line arguments
pub fn run() -> sc_cli::Result<()> {
    let cli = Cli::from_args();

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
    use sc_cli::SubstrateCli;
    use sp_runtime::{
        traits::{Block as _, Hash as _, HashingFor},
        BuildStorage,
    };

    #[test]
    fn dev_flag_is_valid_cli_syntax() {
        let _cli = Cli::try_parse_from(["quip-network-node", "--dev"])
            .expect("--dev should be valid CLI syntax");
    }

    #[test]
    fn local_chain_specs_are_available() {
        assert!(chain_spec::development_chain_spec().is_ok());
        assert!(chain_spec::local_chain_spec().is_ok());
        assert!(chain_spec::local_three_validator_chain_spec().is_ok());
        assert!(chain_spec::validator_rehearsal_chain_spec().is_ok());
    }

    #[test]
    fn testnet_chain_spec_is_available() {
        assert!(chain_spec::quip_testnet_chain_spec().is_ok());
    }

    #[test]
    fn omitted_chain_flag_is_empty_id() {
        let cli = Cli::try_parse_from(["quip-network-node"]).expect("bare argv");
        assert!(!cli.run.shared_params.is_dev());
        assert_eq!(cli.run.shared_params.chain_id(false), "");
    }

    #[test]
    fn live_aliases_load_the_pinned_genesis_and_metadata() {
        type NodeBlock = quip_protocol_runtime::opaque::Block;
        // Independent of the live loader and its alias mapping. This pin comes
        // from the published network, not a hash generated by the current build.
        let expected_hash: sp_core::H256 =
            "542a9915acf840b8940d35ca6de242f669a0c6e3b40f9a8d8c14b9bf7b8ebecb"
                .parse()
                .unwrap();
        let canonical = chain_spec::ChainSpec::from_json_bytes(
            include_bytes!("../../chain-specs/aglais-network.json").as_slice(),
        )
        .unwrap();
        let expected_storage = canonical.build_storage().unwrap();
        assert!(expected_storage.children_default.is_empty());
        let expected_json = canonical.as_json(true).unwrap();
        let executor =
            sc_executor::WasmExecutor::<sp_io::SubstrateHostFunctions>::builder().build();
        let state_version =
            sc_service::resolve_state_version_from_wasm::<_, HashingFor<NodeBlock>>(
                &expected_storage,
                &executor,
            )
            .expect("state version comes from the vendored historical runtime");
        let genesis_hash = |storage: sp_core::storage::Storage| {
            assert!(storage.children_default.is_empty());
            let root = HashingFor::<NodeBlock>::trie_root(
                storage.top.into_iter().collect(),
                state_version,
            );
            sc_service::construct_genesis_block::<NodeBlock>(root, state_version)
                .header()
                .hash()
        };
        assert_eq!(genesis_hash(expected_storage.clone()), expected_hash);

        for alias in ["", "quip-testnet", "quip_testnet", "testnet"] {
            let args = if alias.is_empty() {
                vec!["quip-network-node"]
            } else {
                vec!["quip-network-node", "--chain", alias]
            };
            let cli = Cli::try_parse_from(args).unwrap();
            let chain_id = cli.run.shared_params.chain_id(false);
            assert_eq!(chain_id, alias);
            let spec = cli.load_spec(&chain_id).expect("live alias loads");
            let storage = spec.build_storage().unwrap();
            assert!(
                storage.top == expected_storage.top,
                "raw top differs: {alias}"
            );
            assert!(
                storage.children_default == expected_storage.children_default,
                "raw children differ: {alias}"
            );
            // Full serialized semantics include metadata, bootnodes and code,
            // beyond the storage/genesis identity assertions above.
            assert!(
                spec.as_json(true).unwrap() == expected_json,
                "spec differs: {alias}"
            );
            assert_eq!(genesis_hash(storage), expected_hash, "alias: {alias}");
        }
    }

    #[test]
    fn unprovisioned_networks_have_no_builtin_chain_alias() {
        let cli = Cli::try_parse_from(["quip-network-node"]).unwrap();
        for name in ["quip-mainnet", "quip-testnet-relaunch"] {
            assert!(!std::path::Path::new(name).exists());
            assert!(
                cli.load_spec(name).is_err(),
                "unexpected built-in alias: {name}"
            );
        }
    }
}
