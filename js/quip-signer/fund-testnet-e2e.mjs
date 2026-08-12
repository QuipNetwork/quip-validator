// One-off: fund an EVM (H160) account on quip-testnet by transferring native
// AGLS from a prefunded account to the address's revive-mapped native account
// (h160 ++ 0xEE*12), signed with the Quip hybrid signer.
//
// Usage:
//   QUIP_FUNDER_MNEMONIC="..." node fund-testnet-e2e.mjs
import { ApiPromise, WsProvider } from '@polkadot/api'
import { GenericExtrinsicSignatureV4 } from '@polkadot/types'
import { cryptoWaitReady } from '@polkadot/util-crypto'
import { readFileSync } from 'node:fs'

import {
  DevSeedProvider,
  patchExtrinsicSignFake,
  QuipSigner
} from './dist/index.js'
import initWasm, * as wasm from '../quip-transaction-crypto-wasm/quip_transaction_crypto_wasm.js'

const WS_URL = process.env.QUIP_WS_URL ?? 'wss://bootnode-1.testnet.quip.network:20049/rpc'
const TARGET_H160 = '0x7a718c27469499aae7c652c0d1a95bd14eca4cf9'
// 10 AGLS at 12 native decimals.
const AMOUNT = 10_000_000_000_000n
// pallet-revive AccountId32Mapper fallback: h160 bytes ++ 0xEE * 12.
const DEST_ACCOUNT = `${TARGET_H160}${'ee'.repeat(12)}`

const mnemonic = process.env.QUIP_FUNDER_MNEMONIC
if (!mnemonic) throw new Error('QUIP_FUNDER_MNEMONIC is not set')

await cryptoWaitReady()
const wasmBytes = readFileSync(new URL(
  '../quip-transaction-crypto-wasm/quip_transaction_crypto_wasm_bg.wasm',
  import.meta.url
))
await initWasm({ module_or_path: wasmBytes })
patchExtrinsicSignFake(GenericExtrinsicSignatureV4)

const wsProvider = new WsProvider(WS_URL, false)
await wsProvider.connect()
const api = await ApiPromise.create({ provider: wsProvider })

try {
  const [chain, rv] = await Promise.all([
    api.rpc.system.chain(),
    api.rpc.state.getRuntimeVersion()
  ])
  console.log(`chain: ${chain} spec=${rv.specVersion} tx=${rv.transactionVersion}`)

  const { accounts, provider: secretProvider } = await DevSeedProvider.fromMnemonics(wasm, [{
    name: 'funder',
    mnemonic
  }])
  const [funder] = accounts
  const signer = new QuipSigner(secretProvider)

  const funderInfo = await api.query.system.account(funder.accountIdHex)
  const destBefore = await api.query.system.account(DEST_ACCOUNT)
  console.log(`funder account: ${funder.accountIdHex} free=${funderInfo.data.free}`)
  console.log(`dest (mapped ${TARGET_H160}): ${DEST_ACCOUNT}`)
  console.log(`dest free before: ${destBefore.data.free}`)

  if (BigInt(funderInfo.data.free.toString()) < AMOUNT) {
    throw new Error(`funder balance ${funderInfo.data.free} is below ${AMOUNT}`)
  }

  const tx = api.tx.balances.transferAllowDeath(DEST_ACCOUNT, AMOUNT)
  const inclusion = await new Promise((resolve, reject) => {
    tx.signAndSend(funder.address, { signer }, (result) => {
      if (result.dispatchError) {
        reject(new Error(`dispatch error: ${result.dispatchError.toString()}`))
      } else if (result.status.isInBlock) {
        resolve(result.status.asInBlock.toHex())
      }
    }).catch(reject)
  })

  const destAfter = await api.query.system.account(DEST_ACCOUNT)
  console.log(`included in block: ${inclusion}`)
  console.log(`dest free after: ${destAfter.data.free}`)
} finally {
  await api.disconnect()
}
