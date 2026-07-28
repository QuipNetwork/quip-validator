// @ts-nocheck
// Fund a public, development-only Ethereum account through Alice and pallet-revive.
//
// This helper intentionally connects to an already-running node and sidecar. It
// never starts, stops, purges, or resets either process.

import { ApiPromise, WsProvider } from '@polkadot/api'
import { GenericExtrinsicSignatureV4 } from '@polkadot/types'
import { hexToU8a, u8aToHex } from '@polkadot/util'
import {
  cryptoWaitReady,
  ethereumEncode,
  keccakAsU8a,
  secp256k1PairFromSeed
} from '@polkadot/util-crypto'
import { readFileSync } from 'node:fs'
import { fileURLToPath } from 'node:url'
import { resolve } from 'node:path'

import {
  DevSeedProvider,
  patchExtrinsicSignFake,
  QuipSigner
} from '../dist/index.js'
import initWasm, * as wasm from '../../quip-transaction-crypto-wasm/quip_transaction_crypto_wasm.js'

export const DEV_ETH_ADDRESS = '0x75e480db528101a381ce68544611c169ad7eb342'
export const ALICE_ACCOUNT_ID =
  '0x504c921d4b618d2cbb53ebebfbc98db585b325c355259545739daafb3146cdb4'
export const ALICE_REVIVE_ADDRESS = '0x065e461ede034a4014d9bd5d6c8fa64a2a314ab3'
export const NATIVE_TO_ETH_RATIO = 1_000_000n
export const ED_FLOOR_MULTIPLIER = 100n

const EXPECTED_CHAIN_ID = 1_337n
const EXPECTED_SPEC_NAME = 'quip'
const EXPECTED_SPEC_VERSION = 113
const DEFAULT_REQUEST_TIMEOUT_MS = 10_000
const CALL_WEIGHT_LIMIT = {
  refTime: 1_000_000_000n,
  proofSize: 65_536n
}

// Public deterministic development seeds. Never use either key in production.
const DEV_ETH_SEED_HEX =
  '0xa872f6cbd25a0e04a08b1e21098017a9e6194d101d75e13111f71410c59cd57f'
const ALICE_SEED_HEX =
  '0xe5be9a5092b81bca64be81d212e7f2f9eba183bb7a90954f7b76361f6edb5c0a'

function assert (condition, message) {
  if (!condition) {
    throw new Error(message)
  }
}

function normalizeHex (value) {
  return value.toLowerCase()
}

export function parseQuantity (name, value) {
  assert(
    typeof value === 'string' && /^(?:0x[0-9a-f]+|[0-9]+)$/iu.test(value),
    `${name} must be a non-negative decimal or 0x-prefixed quantity`
  )

  return BigInt(value)
}

export function ceilDiv (numerator, denominator) {
  assert(numerator >= 0n, 'ceilDiv numerator must be non-negative')
  assert(denominator > 0n, 'ceilDiv denominator must be positive')

  return (numerator + denominator - 1n) / denominator
}

export function computeTargets (
  existentialDeposit,
  nativeToEthRatio,
  userMinimumRpc = 0n
) {
  assert(existentialDeposit > 0n, 'existential deposit must be positive')
  assert(nativeToEthRatio > 0n, 'native-to-Ethereum ratio must be positive')
  assert(userMinimumRpc >= 0n, 'user minimum must be non-negative')

  const nativeFloor = existentialDeposit * ED_FLOOR_MULTIPLIER
  const rpcFloor = nativeFloor * nativeToEthRatio

  return {
    effectiveRpcTarget: userMinimumRpc > rpcFloor ? userMinimumRpc : rpcFloor,
    nativeFloor,
    rpcFloor
  }
}

export function fundingDecision (currentRpcBalance, targetRpcBalance, nativeToEthRatio) {
  assert(currentRpcBalance >= 0n, 'current RPC balance must be non-negative')
  assert(targetRpcBalance >= 0n, 'target RPC balance must be non-negative')
  assert(nativeToEthRatio > 0n, 'native-to-Ethereum ratio must be positive')

  if (currentRpcBalance >= targetRpcBalance) {
    return {
      fund: false,
      nativeAmount: 0n,
      representedRpcAmount: 0n,
      rpcShortfall: 0n
    }
  }

  const rpcShortfall = targetRpcBalance - currentRpcBalance
  const nativeAmount = ceilDiv(rpcShortfall, nativeToEthRatio)

  return {
    fund: true,
    nativeAmount,
    representedRpcAmount: nativeAmount * nativeToEthRatio,
    rpcShortfall
  }
}

export function mappingDecision (currentAccountId, expectedAccountId) {
  if (currentAccountId === null) {
    return 'create'
  }

  if (normalizeHex(currentAccountId) === normalizeHex(expectedAccountId)) {
    return 'already-exact'
  }

  throw new Error(
    `revive mapping conflict: expected ${expectedAccountId}, found ${currentAccountId}`
  )
}

export function fallbackAccountId (address) {
  assert(/^0x[0-9a-f]{40}$/iu.test(address), `invalid H160 address: ${address}`)

  return `${normalizeHex(address)}${'ee'.repeat(12)}`
}

export function deriveAliceReviveAddress (accountIdHex = ALICE_ACCOUNT_ID) {
  const accountId = hexToU8a(accountIdHex)

  assert(accountId.length === 32, 'Alice account ID must be 32 bytes')

  return u8aToHex(keccakAsU8a(accountId, 256).subarray(12)).toLowerCase()
}

export function deriveDevEthAddress () {
  const publicKey = secp256k1PairFromSeed(hexToU8a(DEV_ETH_SEED_HEX)).publicKey

  return ethereumEncode(publicKey).toLowerCase()
}

async function jsonRpc (url, method, params = [], timeoutMs = DEFAULT_REQUEST_TIMEOUT_MS) {
  const controller = new AbortController()
  const timeout = setTimeout(() => controller.abort(), timeoutMs)

  try {
    const response = await fetch(url, {
      body: JSON.stringify({
        id: 1,
        jsonrpc: '2.0',
        method,
        params
      }),
      headers: { 'content-type': 'application/json' },
      method: 'POST',
      signal: controller.signal
    })

    assert(response.ok, `${method} at ${url} returned HTTP ${response.status}`)

    const payload = await response.json()

    assert(!payload.error, `${method} at ${url} failed: ${JSON.stringify(payload.error)}`)
    assert(
      Object.prototype.hasOwnProperty.call(payload, 'result'),
      `${method} at ${url} returned no result`
    )

    return payload.result
  } catch (error) {
    if (error.name === 'AbortError') {
      throw new Error(`${method} at ${url} timed out after ${timeoutMs}ms`)
    }

    throw new Error(`${method} at ${url} is unavailable: ${error.message}`)
  } finally {
    clearTimeout(timeout)
  }
}

async function connectApi (wsUrl, timeoutMs = DEFAULT_REQUEST_TIMEOUT_MS) {
  const provider = new WsProvider(wsUrl, false)
  let timeout

  try {
    await provider.connect()

    return await Promise.race([
      ApiPromise.create({ provider }),
      new Promise((_, reject) => {
        timeout = setTimeout(
          () => reject(new Error(`WebSocket endpoint ${wsUrl} timed out after ${timeoutMs}ms`)),
          timeoutMs
        )
      })
    ])
  } catch (error) {
    await provider.disconnect()
    throw error
  } finally {
    clearTimeout(timeout)
  }
}

function requireFunction (value, description) {
  assert(typeof value === 'function', `connected runtime is missing ${description}`)
}

async function validateCapabilities (api, nodeHttpUrl, reviveRpcUrl) {
  const [
    health,
    httpRuntimeVersion,
    chain,
    nodeMethods,
    reviveMethods,
    reviveChainId
  ] = await Promise.all([
    jsonRpc(nodeHttpUrl, 'system_health'),
    jsonRpc(nodeHttpUrl, 'state_getRuntimeVersion'),
    jsonRpc(nodeHttpUrl, 'system_chain'),
    jsonRpc(nodeHttpUrl, 'rpc_methods'),
    jsonRpc(reviveRpcUrl, 'rpc_methods'),
    jsonRpc(reviveRpcUrl, 'eth_chainId')
  ])

  assert(health && typeof health === 'object', 'node HTTP endpoint returned invalid health')
  assert(typeof chain === 'string' && chain.length > 0, 'node returned an empty chain name')
  assert(httpRuntimeVersion.specName === EXPECTED_SPEC_NAME, [
    `node HTTP endpoint is not Quip: expected specName ${EXPECTED_SPEC_NAME},`,
    `got ${httpRuntimeVersion.specName}`
  ].join(' '))
  assert(Number(httpRuntimeVersion.specVersion) === EXPECTED_SPEC_VERSION, [
    `unsupported Quip runtime specVersion ${httpRuntimeVersion.specVersion};`,
    `this helper pins NativeToEthRatio=${NATIVE_TO_ETH_RATIO} for specVersion`,
    `${EXPECTED_SPEC_VERSION}`
  ].join(' '))
  assert(api.runtimeVersion.specName.toString() === EXPECTED_SPEC_NAME, [
    `node WebSocket endpoint is not Quip: expected specName ${EXPECTED_SPEC_NAME},`,
    `got ${api.runtimeVersion.specName}`
  ].join(' '))
  assert(api.runtimeVersion.specVersion.toNumber() === EXPECTED_SPEC_VERSION, [
    `unsupported WebSocket runtime specVersion ${api.runtimeVersion.specVersion};`,
    `expected ${EXPECTED_SPEC_VERSION}`
  ].join(' '))
  assert(parseQuantity('eth_chainId', reviveChainId) === EXPECTED_CHAIN_ID, [
    `revive RPC chain ID mismatch: expected ${EXPECTED_CHAIN_ID},`,
    `got ${reviveChainId}`
  ].join(' '))

  const requiredNodeMethods = ['state_getRuntimeVersion', 'system_health']
  const requiredReviveMethods = ['eth_chainId', 'eth_getBalance']

  for (const method of requiredNodeMethods) {
    assert(nodeMethods.methods?.includes(method), `node RPC is missing ${method}`)
  }
  for (const method of requiredReviveMethods) {
    assert(reviveMethods.methods?.includes(method), `revive RPC is missing ${method}`)
  }

  requireFunction(api.tx.revive?.mapAccount, 'revive.mapAccount')
  requireFunction(api.tx.revive?.call, 'revive.call')
  requireFunction(api.query.revive?.originalAccount, 'revive.originalAccount')
  assert(api.consts.balances?.existentialDeposit, 'runtime is missing balances.existentialDeposit')
  assert(api.events.system?.ExtrinsicSuccess, 'runtime is missing System.ExtrinsicSuccess')
  assert(api.events.system?.ExtrinsicFailed, 'runtime is missing System.ExtrinsicFailed')
  assert(api.events.balances?.Transfer, 'runtime is missing Balances.Transfer')

  return { chain }
}

async function queryOriginalAccount (api, address) {
  const original = await api.query.revive.originalAccount(address)

  return original.isNone ? null : original.unwrap().toHex().toLowerCase()
}

function describeDispatchError (api, dispatchError) {
  if (dispatchError.isModule) {
    const decoded = api.registry.findMetaError(dispatchError.asModule)

    return {
      docs: decoded.docs.join(' '),
      name: decoded.name,
      section: decoded.section
    }
  }

  return {
    docs: '',
    name: dispatchError.type || dispatchError.toString(),
    section: ''
  }
}

async function submitAndInspect (api, tx, account, signer) {
  const payment = await tx.paymentInfo(account.address, { signer })

  assert(payment.partialFee.gtn(0), 'fee estimation returned a zero fee')

  await tx.signAsync(account.address, { nonce: -1, signer })

  const submittedHex = tx.toHex().toLowerCase()
  const txHash = tx.hash.toHex()

  return await new Promise((resolvePromise, rejectPromise) => {
    let unsubscribe = () => undefined
    let finished = false
    let isCheckingBlock = false
    const submissionTimeout = setTimeout(() => {
      finished = true
      unsubscribe()
      rejectPromise(new Error(`transaction ${txHash} was not included within 120000ms`))
    }, 120_000)

    const resolveOnce = (result) => {
      if (finished) {
        return
      }

      finished = true
      clearTimeout(submissionTimeout)
      unsubscribe()
      resolvePromise(result)
    }

    const rejectOnce = (error) => {
      if (finished) {
        return
      }

      finished = true
      clearTimeout(submissionTimeout)
      unsubscribe()
      rejectPromise(error)
    }

    api.rpc.author.submitAndWatchExtrinsic.raw(submittedHex, (rawStatus) => {
      if (finished) {
        return
      }

      const status = api.createType('ExtrinsicStatus', rawStatus)

      if (status.isInvalid || status.isDropped || status.isUsurped || status.isFinalityTimeout) {
        rejectOnce(new Error(`transaction submission failed with ${status.type}`))
      } else if (status.isInBlock && !isCheckingBlock) {
        isCheckingBlock = true

        const blockHash = status.asInBlock

        void (async () => {
          // The current runtime can author bare V5 inherents that polkadot-js
          // 16 cannot decode as V4 Extrinsics. Match inclusion from raw hex.
          const rawBlock = await api.rpc.chain.getBlock.raw(blockHash)
          const extrinsics = rawBlock?.block?.extrinsics

          assert(Array.isArray(extrinsics), 'raw block did not contain an extrinsics array')

          const extrinsicIndex = extrinsics.findIndex((encoded) =>
            typeof encoded === 'string' && encoded.toLowerCase() === submittedHex
          )

          assert(extrinsicIndex !== -1, 'submitted extrinsic was not found in the raw block')

          const blockApi = await api.at(blockHash)
          const allRecords = await blockApi.query.system.events()
          const records = allRecords.filter(({ phase }) =>
            phase.isApplyExtrinsic && phase.asApplyExtrinsic.toNumber() === extrinsicIndex
          )
          const success = records.some(({ event }) =>
            api.events.system.ExtrinsicSuccess.is(event)
          )
          const failureRecord = records.find(({ event }) =>
            api.events.system.ExtrinsicFailed.is(event)
          )
          const dispatchError = failureRecord
            ? describeDispatchError(api, failureRecord.event.data[0])
            : null
          const header = await api.rpc.chain.getHeader(blockHash)

          assert(
            success || dispatchError,
            `extrinsic ${extrinsicIndex} had neither ExtrinsicSuccess nor ExtrinsicFailed`
          )

          return {
            blockHash: blockHash.toHex(),
            blockNumber: header.number.toNumber(),
            dispatchError,
            records,
            success,
            txHash
          }
        })()
          .then(resolveOnce)
          .catch(rejectOnce)
      }
    })
      .then((unsub) => {
        unsubscribe = unsub

        if (finished) {
          unsubscribe()
        }
      })
      .catch(rejectOnce)
  })
}

async function ensureAliceMapping (api, alice, signer, mappedAliceAddress) {
  const before = await queryOriginalAccount(api, mappedAliceAddress)
  const decision = mappingDecision(before, alice.accountIdHex)

  if (decision === 'already-exact') {
    return { action: 'already exact; skipped', inclusion: null }
  }

  const inclusion = await submitAndInspect(api, api.tx.revive.mapAccount(), alice, signer)
  const after = await queryOriginalAccount(api, mappedAliceAddress)

  if (inclusion.success) {
    assert(
      mappingDecision(after, alice.accountIdHex) === 'already-exact',
      'mapAccount succeeded but the exact Alice reverse mapping was not stored'
    )

    return { action: 'created', inclusion }
  }

  const alreadyMapped = inclusion.dispatchError?.section === 'revive' &&
    inclusion.dispatchError?.name === 'AccountAlreadyMapped'

  if (alreadyMapped && mappingDecision(after, alice.accountIdHex) === 'already-exact') {
    return { action: 'already exact after AccountAlreadyMapped race', inclusion }
  }

  const errorName = inclusion.dispatchError
    ? `${inclusion.dispatchError.section}.${inclusion.dispatchError.name}`
    : 'unknown dispatch error'

  throw new Error(`revive.mapAccount failed with ${errorName}`)
}

function hasMatchingTransfer (api, records, fromAccountId, toAccountId, amount) {
  return records.some(({ event }) => {
    if (!api.events.balances.Transfer.is(event)) {
      return false
    }

    const [from, to, transferred] = event.data

    return normalizeHex(from.toHex()) === normalizeHex(fromAccountId) &&
      normalizeHex(to.toHex()) === normalizeHex(toAccountId) &&
      BigInt(transferred.toString()) === amount
  })
}

function printInclusion (label, inclusion) {
  if (!inclusion) {
    return
  }

  console.log(`${label} extrinsic hash: ${inclusion.txHash}`)
  console.log(
    `${label} included: block #${inclusion.blockNumber} (${inclusion.blockHash})`
  )
}

export async function main () {
  const wsUrl = process.env.QUIP_WS_URL || 'ws://127.0.0.1:9944'
  const nodeHttpUrl = process.env.QUIP_HTTP_URL || 'http://127.0.0.1:9944'
  const reviveRpcUrl =
    process.env.REVIVE_RPC_URL || process.env.ETH_RPC_URL || 'http://127.0.0.1:8545'
  const userMinimumRpc = process.env.QUIP_REVIVE_MIN_RPC_BALANCE
    ? parseQuantity('QUIP_REVIVE_MIN_RPC_BALANCE', process.env.QUIP_REVIVE_MIN_RPC_BALANCE)
    : 0n

  await cryptoWaitReady()

  const devAddress = deriveDevEthAddress()
  const mappedAliceAddress = deriveAliceReviveAddress()

  assert(
    devAddress === DEV_ETH_ADDRESS,
    `development Ethereum address drifted: expected ${DEV_ETH_ADDRESS}, got ${devAddress}`
  )
  assert(
    mappedAliceAddress === ALICE_REVIVE_ADDRESS,
    `Alice revive address drifted: expected ${ALICE_REVIVE_ADDRESS}, got ${mappedAliceAddress}`
  )

  console.log('WARNING: using a hard-coded public development-only ECDSA key')
  console.log('Never use this account or key material in production')
  console.log(`dev H160: ${devAddress}`)

  const api = await connectApi(wsUrl)

  try {
    // Finish all compatibility checks before the first possible mutation.
    const { chain } = await validateCapabilities(api, nodeHttpUrl, reviveRpcUrl)

    console.log(`connected Quip chain: ${chain}`)
    console.log(`revive chain ID: ${EXPECTED_CHAIN_ID}`)
    console.log(`NativeToEthRatio: ${NATIVE_TO_ETH_RATIO}`)

    const wasmBytes = readFileSync(new URL(
      '../../quip-transaction-crypto-wasm/quip_transaction_crypto_wasm_bg.wasm',
      import.meta.url
    ))

    await initWasm({ module_or_path: wasmBytes })
    patchExtrinsicSignFake(GenericExtrinsicSignatureV4)

    const { accounts, provider } = await DevSeedProvider.fromSeeds(wasm, [{
      name: 'Quip Alice',
      seedHex: ALICE_SEED_HEX
    }])
    const [alice] = accounts

    assert(
      normalizeHex(alice.accountIdHex) === ALICE_ACCOUNT_ID,
      `Alice account ID drifted: expected ${ALICE_ACCOUNT_ID}, got ${alice.accountIdHex}`
    )

    const signer = new QuipSigner(provider)
    const devOriginalAccount = await queryOriginalAccount(api, devAddress)

    assert(
      devOriginalAccount === null,
      [
        `development H160 ${devAddress} unexpectedly has a native reverse mapping`,
        `to ${devOriginalAccount}; refusing to fund an ambiguous destination`
      ].join(' ')
    )

    const mapping = await ensureAliceMapping(api, alice, signer, mappedAliceAddress)

    console.log(`Alice mapped H160: ${mappedAliceAddress}`)
    console.log(`Alice mapping action: ${mapping.action}`)
    printInclusion('mapping', mapping.inclusion)

    const existentialDeposit = BigInt(api.consts.balances.existentialDeposit.toString())
    const targets = computeTargets(
      existentialDeposit,
      NATIVE_TO_ETH_RATIO,
      userMinimumRpc
    )

    console.log(`existential deposit (native): ${existentialDeposit}`)
    console.log(`100x ED target (native): ${targets.nativeFloor}`)
    console.log(`100x ED target (RPC): ${targets.rpcFloor}`)
    console.log(`effective RPC target: ${targets.effectiveRpcTarget}`)

    const before = parseQuantity(
      'eth_getBalance',
      await jsonRpc(reviveRpcUrl, 'eth_getBalance', [devAddress, 'latest'])
    )
    const decision = fundingDecision(
      before,
      targets.effectiveRpcTarget,
      NATIVE_TO_ETH_RATIO
    )

    console.log(`RPC balance before: ${before}`)

    let fundingInclusion = null

    if (decision.fund) {
      // pallet-revive treats account-creation ED as a separate storage deposit.
      // The call value remains exactly the rounded-up spendable shortfall.
      const tx = api.tx.revive.call(
        devAddress,
        decision.nativeAmount,
        CALL_WEIGHT_LIMIT,
        existentialDeposit,
        '0x'
      )

      fundingInclusion = await submitAndInspect(api, tx, alice, signer)

      if (!fundingInclusion.success) {
        const errorName = fundingInclusion.dispatchError
          ? `${fundingInclusion.dispatchError.section}.${fundingInclusion.dispatchError.name}`
          : 'unknown dispatch error'

        throw new Error(`revive.call failed with ${errorName}`)
      }

      assert(
        hasMatchingTransfer(
          api,
          fundingInclusion.records,
          alice.accountIdHex,
          fallbackAccountId(devAddress),
          decision.nativeAmount
        ),
        [
          'revive.call succeeded without the expected Balances.Transfer',
          `of ${decision.nativeAmount} native units to ${fallbackAccountId(devAddress)}`
        ].join(' ')
      )
    }

    const after = parseQuantity(
      'eth_getBalance',
      await jsonRpc(reviveRpcUrl, 'eth_getBalance', [devAddress, 'latest'])
    )

    assert(
      after >= targets.effectiveRpcTarget,
      `final RPC balance ${after} is below effective target ${targets.effectiveRpcTarget}`
    )

    console.log(`amount funded (native): ${decision.nativeAmount}`)
    console.log(`represented funding (RPC): ${decision.representedRpcAmount}`)
    printInclusion('funding', fundingInclusion)
    console.log(`RPC balance after: ${after}`)
    console.log(
      decision.fund
        ? 'funding action: submitted required shortfall'
        : 'funding action: already funded; skipped'
    )
    console.log('revive development account funding passed')
  } finally {
    await api.disconnect()
  }
}

const isMain = process.argv[1] &&
  resolve(process.argv[1]) === resolve(fileURLToPath(import.meta.url))

if (isMain) {
  await main().catch((error) => {
    console.error(`revive funding failed: ${error.message}`)
    process.exitCode = 1
  })
}
