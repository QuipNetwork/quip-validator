# BitGo custody integration

Runtime 117 exposes the custody pallets at stable indices: Multisig 16,
Utility 17, and Proxy 18. The signed-extrinsic format remains H4 and
`transaction_version` remains 7.

## Can one transaction send several transfers?

Yes. Submit the balance-transfer calls inside `Utility.batch_all`. Every call
uses the outer signer's origin and the operation is atomic: if any transfer
fails, all earlier transfers in that batch are rolled back. `Utility.batch`
is the non-atomic alternative when partial completion is intentional.

`Multisig.as_multi` may carry the same `Utility.batch_all` call, so a custody
quorum can authorize a group of transfers as one operation. For a 2-of-3
workflow, intermediate approval should use `approve_as_multi` with only the
call hash; the final signer submits `as_multi` with the full batch call.

## Are Substrate derivation junctions supported by the H4 signer?

No. `master_seed_from_secret_uri` accepts a 32-byte hex seed or an English
BIP39 phrase, optionally followed by `///<password>`. It deliberately rejects
hard (`//...`) and soft (`/...`) derivation junctions: any slash outside the
triple-slash password separator returns `UnsupportedDerivationPath`. A custody
integration must import distinct H4 seeds rather than relying on Substrate
derivation paths.

## How does `Utility.as_derivative` work?

`as_derivative(index, call)` dispatches the inner call as the deterministic
pseudonym

`blake2_256(SCALE("modlpy/utilisuba", owner AccountId, index u16))`.

The owner remains the only key needed to submit the outer H4 transaction, but
the derivative account has its own balance and nonce-bearing account state.
For the H4 `//Alice` test account and index 7, the runtime derives
`5EZbeudbHoiTHk6bJHxcpsC1ppDB2knxwKq4FMWKTMcJYEDG`.

This derived `AccountId32` is not Ethereum-derived. With the runtime's Revive
`AccountId32Mapper`, its H160 address is a lossy Keccak hash unless a reverse
mapping is stored. Before the derivative account originates Revive contract
interactions, fund it and have it submit the permissionless, self-signed
`Revive.map_account` call (call index 7). The mapping holds
`DepositPerItem + 52 * DepositPerByte`, currently 200,520,000,000 plancks
(0.20052 QUIP), until the account calls `Revive.unmap_account`.

## Proxy policy

The runtime offers `Any` and `TransferOnly` proxy types. `TransferOnly` admits
only `Balances.transfer_allow_death`, `Balances.transfer_keep_alive`, and
`Balances.transfer_all`; it rejects utility nesting, proxy administration,
contracts, sudo, and all other calls. Delayed proxies use `announce` and
`proxy_announced`, and the real account may reject an announcement before its
delay expires.

`Proxy.create_pure` is enabled for the standard custody pattern where a key
holder creates an otherwise inaccessible vault account controlled solely by
its proxy relationship. Record the `PureCreated` event fields needed by
`kill_pure`; losing them can make safe teardown impossible.
