// Destructive ONLY to a disposable local3 rehearsal chain. Start three nodes
// with the Phase 2 binary, then run this script. It removes Bob from admission
// and verifies finalized state across the resulting BABE/GRANDPA set change.
// Requires the existing wasm signer artifacts and @polkadot/api dependencies.
import assert from 'node:assert/strict';
import { readFileSync } from 'node:fs';
import { setTimeout as delay } from 'node:timers/promises';
import { ApiPromise, WsProvider } from '@polkadot/api';
import { GenericExtrinsicSignatureV4 } from '@polkadot/types';
import { addressEq } from '@polkadot/util-crypto';
import { DevSeedProvider, QuipSigner, patchExtrinsicSignFake } from '../dist/index.js';
import initWasm, * as wasm from '../../quip-transaction-crypto-wasm/quip_transaction_crypto_wasm.js';

const endpoint = process.env.QUIP_WS_URL || 'ws://127.0.0.1:9944';
assert(['127.0.0.1', 'localhost', '[::1]'].includes(new URL(endpoint).hostname), 'rehearsal requires loopback RPC');
await initWasm({ module_or_path: readFileSync(new URL('../../quip-transaction-crypto-wasm/quip_transaction_crypto_wasm_bg.wasm', import.meta.url)) });
patchExtrinsicSignFake(GenericExtrinsicSignatureV4);
const { accounts, provider } = await DevSeedProvider.fromSeeds(wasm, [
  { name: 'Alice', seedHex: '0xe5be9a5092b81bca64be81d212e7f2f9eba183bb7a90954f7b76361f6edb5c0a' },
  { name: 'Bob', seedHex: '0x398f0c28f98885e046333d4a41c19cee4c37368a9832c6502f6cfd182e2aef89' }
]);
const [alice, bob] = accounts;
const signer = new QuipSigner(provider);
const api = await ApiPromise.create({ provider: new WsProvider(endpoint) });

async function propose(call, needsRootResult = false) {
  const tx = api.tx.foundation.propose(1, call.method, call.method.encodedLength);
  await tx.signAsync(alice.address, { nonce: -1, signer });
  const hex = tx.toHex().toLowerCase();
  await new Promise((resolve, reject) => {
    let unsubscribe;
    let done = false;
    const finish = (error) => {
      if (done) return;
      done = true;
      clearTimeout(timer);
      unsubscribe?.();
      error ? reject(error) : resolve();
    };
    const timer = setTimeout(() => finish(new Error('finalized proposal receipt timed out')), 180_000);
    api.rpc.author.submitAndWatchExtrinsic.raw(hex, raw => {
      const status = api.createType('ExtrinsicStatus', raw);
      if (status.isInvalid || status.isDropped || status.isUsurped || status.isFinalityTimeout) {
        finish(new Error(`proposal ${status.type}`));
      } else if (status.isFinalized && !done) {
        (async () => {
          const hash = status.asFinalized;
          const block = await api.rpc.chain.getBlock.raw(hash);
          const index = block.block.extrinsics.findIndex(value => value.toLowerCase() === hex);
          assert(index >= 0, 'proposal missing from finalized block');
          const at = await api.at(hash);
          const events = (await at.query.system.events()).filter(({ phase }) => phase.isApplyExtrinsic && phase.asApplyExtrinsic.toNumber() === index).map(({ event }) => event);
          assert(events.some(e => e.section === 'system' && e.method === 'ExtrinsicSuccess'), 'outer dispatch did not succeed');
          const executed = events.find(e => e.section === 'foundation' && e.method === 'Executed');
          assert(executed && executed.data[1].isOk, 'collective inner dispatch did not succeed');
          if (needsRootResult) {
            const root = events.find(e => e.section === 'validatorAdmission' && e.method === 'RootDispatched');
            assert(root && root.data[0].isOk, 'governed Root call did not succeed');
          }
        })().then(() => finish(), finish);
      }
    }).then(unsub => { unsubscribe = unsub; if (done) unsub(); }).catch(finish);
  });
}

try {
  const chain = (await api.rpc.system.chain()).toString();
  assert.equal(chain, 'Local Testnet (3 Validators)', 'use a fresh disposable local3 chain');
  const beforeHash = await api.rpc.chain.getFinalizedHead();
  const before = await api.at(beforeHash);
  assert.equal((await before.query.session.validators()).length, 3);
  const members = await before.query.foundation.members();
  assert.equal(members.length, 1, 'script assumes the local preset one-member Foundation');
  assert(addressEq(members[0].toString(), alice.address));
  const oldSet = await before.query.grandpa.currentSetId();
  await propose(api.tx.validatorAdmission.remove(bob.address));
  const admin = api.tx.utility.batchAll([
    api.tx.staking.setValidatorCount(2), api.tx.staking.forceNewEra()
  ]);
  await propose(api.tx.validatorAdmission.dispatchAsRoot(admin.method), true);
  // Epochs remain production length (10 minutes); allow election + queued activation.
  const deadline = Date.now() + 25 * 60_000;
  let changedAt;
  while (Date.now() < deadline) {
    const hash = await api.rpc.chain.getFinalizedHead();
    const at = await api.at(hash);
    const validators = await at.query.session.validators();
    const set = await at.query.grandpa.currentSetId();
    if (validators.length === 2 && set.gt(oldSet)) {
      assert(!validators.some(who => addressEq(who.toString(), bob.address)));
      assert.equal((await at.query.babe.authorities()).length, 2);
      assert.equal((await at.query.grandpa.authorities()).length, 2);
      changedAt = (await api.rpc.chain.getHeader(hash)).number;
      break;
    }
    await delay(5000);
  }
  assert(changedAt, 'no finalized validator-set change within two epochs');
  const deadlineAfter = Date.now() + 180_000;
  let finalNumber = changedAt;
  while (Date.now() < deadlineAfter && finalNumber.lte(changedAt.addn(3))) {
    await delay(5000);
    finalNumber = (await api.rpc.chain.getHeader(await api.rpc.chain.getFinalizedHead())).number;
  }
  assert(finalNumber.gt(changedAt.addn(3)), 'finality stopped after authority change');
  console.log(`PASS: finalized set changed at ${changedAt}; finality advanced to ${finalNumber}`);
} finally {
  await api.disconnect();
}
