# quip-testnet Genesis Manifest

Public material for the three bootnode operators pinned into the
`quip-testnet` genesis preset (introduced in v0.2.0). The full BABE / GRANDPA
public bytes live as `include_str!`-loaded hex blobs under
`runtime/src/genesis_quip_testnet/`. This file is the human-readable index.

Runtime 116 changes consensus authorities to H4/H2 FN-DSA-512 keys (929 bytes
each). Because the previous H3/H1 public material could not be converted, the
three operators derived replacement public bundles from their existing
mnemonics. Runtime 116 pins those rotated keys and accounts while retaining the
existing bootnode peer IDs and multiaddrs; operator 1 remains the sudo account.

## Operators

### Operator 1

- **multiaddr**: `/dns4/bootnode-1.testnet.quip.network/tcp/30333/p2p/12D3KooWBdhB4xGX6hfFsNufqQsG99kekiH9kJhLSiui3RgatnpE`
- **peer-id**: `12D3KooWBdhB4xGX6hfFsNufqQsG99kekiH9kJhLSiui3RgatnpE`
- **tx_account_ss58**: `5GXztdK6VVJYoWVyDqd8macEUgUkfssLYDMx3jvBezMEYHaD`
- **tx_account_hex**: `0xc5c1e685e181d2939fddaf43e61455c0f00a99d2716d59b1f90674e5c0292510`
- **roles (v0.2.0)**: validator session keys, sudo, and faucet dispense
  source — the public testnet faucet in `nodes.quip.network` is configured
  with operator-1's mnemonic. Operator-1's host must therefore safeguard the
  mnemonic with care: any leak compromises validator authority, sudo, and
  faucet funds simultaneously. A future release should split sudo into a
  multisig and the faucet into its own genesis-endowed account.

### Operator 2

- **multiaddr**: `/dns4/bootnode-2.testnet.quip.network/tcp/30333/p2p/12D3KooWPJAHo45AA94u3fYS3tXvyKouZnWihQnXWPHAzikXLfPW`
- **peer-id**: `12D3KooWPJAHo45AA94u3fYS3tXvyKouZnWihQnXWPHAzikXLfPW`
- **tx_account_ss58**: `5DXsz14EEATRP9EXR4q8DPAgooWurBJXGQ61RagJVC9Xn6RA`
- **tx_account_hex**: `0x40f5f2aeab073dfa95c96e48eba57b4afbdd2d118ff4acf60987a8c8bf8bf32b`

### Operator 3

- **multiaddr**: `/dns4/bootnode-3.testnet.quip.network/tcp/30333/p2p/12D3KooWM6n7wYvett975UnLYXrvnBGqLk2DLJoCRoFxgXTkptWe`
- **peer-id**: `12D3KooWM6n7wYvett975UnLYXrvnBGqLk2DLJoCRoFxgXTkptWe`
- **tx_account_ss58**: `5GxchWH8HD3fvNXvULs75n8p65qQjwXxjYH9ZARjCyXhNsPH`
- **tx_account_hex**: `0xd88854de054c7534450cddace65332a98d12ba06ff3adb39f9ad40f129b37aa7`

## Verifying

To independently confirm the rotated genesis preset matches this manifest:

```bash
cargo build --release -p quip-network-node
./target/release/quip-network-node export-chain-spec --chain quip-testnet \
    | jq '.bootNodes, .properties'
```

Expected output: three `/dns4/bootnode-N.testnet.quip.network/.../p2p/12D3KooW…`
multiaddrs (matching the peer-ids above) and `tokenSymbol=AGLS`,
`tokenDecimals=12`, `ss58Format=42`.

To compare the runtime-derived authority public bytes against the hex blobs:

```bash
cargo test -p quip-protocol-runtime --lib genesis_config_presets::tests
```

The `quip_testnet_operator_1_account_is_pinned` test asserts that operator 1's
rotated H4 transaction account round-trips to the pinned bytes, catching silent
regressions in either hex parsing or account-id derivation.

## Updating

To add or rotate an operator, follow [`docs/testnet-keys.md`](testnet-keys.md):
the helper script produces the `public-bundle.txt` and the coordinator
commits the matching `*.hex` files plus a manifest entry here. Bumping the
`quip_testnet` genesis requires re-exporting `quip-testnet.json` and
republishing it in `nodes.quip.network`.
