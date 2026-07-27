# Testnet Key Generation (Operator Procedure)

This document is the canonical procedure for generating the keys that pin a
new `quip-testnet` bootnode operator into the genesis preset. Use it when
rotating an operator or onboarding additional slots beyond the initial three
shipped in v0.2.0.

> **Secret material never leaves the operator host.** The mnemonic and the
> libp2p `nodekey` file are the only secrets in the process. Only the
> `public-bundle.txt` block is submitted back to the release coordinator.

## Prerequisites

1. Clone `quip-validator` at the release-candidate tag.
2. Build the node binary:

   ```bash
   cargo build --release -p quip-network-node
   ```

   On macOS, `.cargo/config.toml` already wires the rpath needed to resolve
   `libclang.dylib` (required by the bindgen-driven `librocksdb-sys` build
   script). No manual `LIBCLANG_PATH` export is needed.

## Per-operator key generation

For each operator slot, run the helper script with the slot index (and
optionally the bootnode hostname). The script wraps the full procedure:

```bash
scripts/derive-operator-keys.sh 1
scripts/derive-operator-keys.sh 2 bootnode-2.testnet.quip.network
scripts/derive-operator-keys.sh 3
```

Each run produces, under `quip-testnet-keys/operator-<N>/`:

| File | Permissions | Purpose |
| --- | --- | --- |
| `mnemonic` | `0600` | BIP39 master secret (back up, never share) |
| `nodekey` | `0600` | libp2p secret bytes; mount via `--node-key-file` on the bootnode host |
| `public-bundle.txt` | `0644` | Public peer-id, multiaddr, BABE/GRANDPA/TX public bytes |

The script auto-creates a `.gitignore` in `quip-testnet-keys/` containing `*`
so secrets cannot accidentally be `git add`'d. The repo's `.gitignore` also
excludes the entire directory.

## Submission

Send the contents of `public-bundle.txt` for each operator to the release
coordinator over a secure channel (signed message in the ops chat, encrypted
mail, etc.). The bundle is:

```
operator: <N>
hostname: <dns>
multiaddr: /dns4/<dns>/tcp/30333/p2p/<peer-id>
peer_id: 12D3KooW…
babe_pub: 0x…             # 1344 bytes hex (sr25519 + ML-DSA-44)
grandpa_pub: 0x…          # 1344 bytes hex (ed25519 + ML-DSA-44)
tx_account_ss58: 5…
tx_account_hex: 0x…       # 32 bytes
```

The coordinator commits the public bytes into
`runtime/src/genesis_quip_testnet/` (BABE/GRANDPA, one hex file per pubkey)
and into the inline `tx_account_from_hex(...)` calls in
`runtime/src/genesis_config_presets.rs::quip_testnet_config_genesis`.

## Manual derivation

Should the helper script be unavailable, the equivalent commands are:

```bash
# 1. Mnemonic
./target/release/quip-network-node key generate
#    → copy the "Secret phrase:" line

# 2. libp2p node-key file
./target/release/quip-network-node key generate-node-key --file ./nodekey
chmod 600 ./nodekey
./target/release/quip-network-node key inspect-node-key --file ./nodekey
#    → prints the peer-id

# 3. Hybrid BABE/GRANDPA/TX public material
cd crates/transaction-crypto
cargo run --release --example derive_genesis_keys --features std -- "<mnemonic>"
```

The example binary lives at
`crates/transaction-crypto/examples/derive_genesis_keys.rs` and produces the
same `babe_pub` / `grandpa_pub` / `tx_account_*` values regardless of whether
it is invoked directly or via the script.
