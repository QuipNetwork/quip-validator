import assert from 'node:assert/strict'

import { cryptoWaitReady } from '@polkadot/util-crypto'

import {
  ALICE_ACCOUNT_ID,
  ALICE_REVIVE_ADDRESS,
  ceilDiv,
  computeTargets,
  deriveAliceReviveAddress,
  deriveDevEthAddress,
  DEV_ETH_ADDRESS,
  fallbackAccountId,
  fundingDecision,
  mappingDecision,
  NATIVE_TO_ETH_RATIO,
  parseQuantity
} from './revive-fund-account.mjs'

await cryptoWaitReady()

assert.equal(deriveDevEthAddress(), DEV_ETH_ADDRESS)
assert.equal(deriveAliceReviveAddress(ALICE_ACCOUNT_ID), ALICE_REVIVE_ADDRESS)
assert.equal(
  fallbackAccountId(DEV_ETH_ADDRESS),
  `${DEV_ETH_ADDRESS}${'ee'.repeat(12)}`
)

assert.equal(mappingDecision(null, ALICE_ACCOUNT_ID), 'create')
assert.equal(mappingDecision(ALICE_ACCOUNT_ID.toUpperCase(), ALICE_ACCOUNT_ID), 'already-exact')
assert.throws(
  () => mappingDecision(`0x${'11'.repeat(32)}`, ALICE_ACCOUNT_ID),
  /revive mapping conflict/u
)

const targets = computeTargets(1_000_000_000n, NATIVE_TO_ETH_RATIO)

assert.deepEqual(targets, {
  effectiveRpcTarget: 100_000_000_000_000_000n,
  nativeFloor: 100_000_000_000n,
  rpcFloor: 100_000_000_000_000_000n
})
assert.equal(
  computeTargets(1_000_000_000n, NATIVE_TO_ETH_RATIO, 1n).effectiveRpcTarget,
  targets.rpcFloor
)
assert.equal(
  computeTargets(
    1_000_000_000n,
    NATIVE_TO_ETH_RATIO,
    targets.rpcFloor + 1n
  ).effectiveRpcTarget,
  targets.rpcFloor + 1n
)

assert.equal(ceilDiv(0n, NATIVE_TO_ETH_RATIO), 0n)
assert.equal(ceilDiv(1n, NATIVE_TO_ETH_RATIO), 1n)
assert.equal(ceilDiv(NATIVE_TO_ETH_RATIO, NATIVE_TO_ETH_RATIO), 1n)
assert.equal(ceilDiv(NATIVE_TO_ETH_RATIO + 1n, NATIVE_TO_ETH_RATIO), 2n)

assert.deepEqual(
  fundingDecision(targets.rpcFloor, targets.rpcFloor, NATIVE_TO_ETH_RATIO),
  {
    fund: false,
    nativeAmount: 0n,
    representedRpcAmount: 0n,
    rpcShortfall: 0n
  }
)
assert.deepEqual(
  fundingDecision(targets.rpcFloor - 1n, targets.rpcFloor, NATIVE_TO_ETH_RATIO),
  {
    fund: true,
    nativeAmount: 1n,
    representedRpcAmount: NATIVE_TO_ETH_RATIO,
    rpcShortfall: 1n
  }
)

assert.equal(parseQuantity('decimal', '1337'), 1_337n)
assert.equal(parseQuantity('hex', '0x539'), 1_337n)
assert.throws(() => parseQuantity('invalid', '-1'), /must be a non-negative/u)

console.log('revive funding unit tests passed')
