// Produce a finalized transfer from the shared H4 fixture seed on a disposable
// --dev node. Uses the existing signer/WASM; no custom decoding types.
import assert from 'node:assert/strict';
import { readFileSync } from 'node:fs';
import { setTimeout as delay } from 'node:timers/promises';
import { ApiPromise, WsProvider } from '@polkadot/api';
import { compactToU8a, hexToU8a, u8aConcat, u8aToHex } from '@polkadot/util';
import { DevSeedProvider, QuipSigner } from '../dist/index.js';
import initWasm, * as wasm from '../../quip-transaction-crypto-wasm/quip_transaction_crypto_wasm.js';

const url = new URL(process.env.QUIP_WS_URL || 'ws://127.0.0.1:19944');
assert(['127.0.0.1', 'localhost', '[::1]'].includes(url.hostname), 'disposable localhost node required');
const fixture = JSON.parse(readFileSync(new URL('../../../docs/polkadotjs/fixtures/hybrid-signing.json', import.meta.url)));
await initWasm({ module_or_path: readFileSync(new URL('../../quip-transaction-crypto-wasm/quip_transaction_crypto_wasm_bg.wasm', import.meta.url)) });
const { accounts: [alice, fixtureAccount, destination], provider } = await DevSeedProvider.fromSeeds(wasm, [
  { name: 'Dev Alice', seedHex: '0xe5be9a5092b81bca64be81d212e7f2f9eba183bb7a90954f7b76361f6edb5c0a' },
  { name: 'Shared signing fixture', seedHex: fixture.seedHex },
  { name: 'Conformance destination', seedHex: '0x' + '09'.repeat(32) }
]);
assert.equal(fixtureAccount.accountIdHex, fixture.accountIdHex);
const signer = new QuipSigner(provider);
const api = await ApiPromise.create({ provider: new WsProvider(url.href), noInitWarn: true });
const eventsKey = '0x26aa394eea5630e07c48ae0c9558cef780d41e5e16056765bc8461851072c9d7';
const deadline = Date.now() + 180_000;

async function transfer(account, dest, amount) {
  const call = api.tx.balances.transferKeepAlive(dest, amount).method;
  const { nonce } = await api.query.system.account(account.address);
  const payload = api.createType('ExtrinsicPayload', {
    method: call.toHex(), era: '0x00', nonce, tip: 0, mode: 0, metadataHash: null,
    specVersion: api.runtimeVersion.specVersion,
    transactionVersion: api.runtimeVersion.transactionVersion,
    genesisHash: api.genesisHash, blockHash: api.genesisHash
  }, { version: 4 });
  const { signature } = await signer.signRaw({ address: account.address, data: u8aToHex(payload.toU8a({ method: true })), type: 'bytes' });
  assert.equal(hexToU8a(signature).length, 1660);
  // Current native V4 wire layout. This is a producer only; consumer tests
  // must decode the resulting bytes exclusively from published metadata.
  const body = u8aConcat(new Uint8Array([0x84, 0]), hexToU8a(account.accountIdHex), hexToU8a(signature),
    new Uint8Array([0]), compactToU8a(nonce), compactToU8a(0), new Uint8Array([0]), call.toU8a());
  const encoded = u8aToHex(u8aConcat(compactToU8a(body.length), body));
  let next = (await api.rpc.chain.getHeader()).number.toNumber() + 1;
  await api.rpc.author.submitExtrinsic.raw(encoded);
  while (Date.now() < deadline) {
    const finalizedHash = await api.rpc.chain.getFinalizedHead();
    const finalized = (await api.rpc.chain.getHeader(finalizedHash)).number.toNumber();
    while (next <= finalized) {
      const hash = await api.rpc.chain.getBlockHash(next);
      const { block } = await api.rpc.chain.getBlock.raw(hash);
      const index = block.extrinsics.indexOf(encoded);
      if (index >= 0) {
        const parentApi = await api.at(block.header.parentHash);
        const eventsHex = await api.rpc.state.getStorage.raw(eventsKey, hash);
        const events = parentApi.registry.createType('Vec<EventRecord>', eventsHex);
        assert(events.some(({ phase, event }) => phase.isApplyExtrinsic && phase.asApplyExtrinsic.toNumber() === index && event.section === 'system' && event.method === 'ExtrinsicSuccess'), `transfer must succeed: ${JSON.stringify(events.toHuman())}`);
        assert(events.some(({ phase, event }) => phase.isApplyExtrinsic && phase.asApplyExtrinsic.toNumber() === index && event.section === 'balances' && event.method === 'Transfer' && event.data[0].toHex() === account.accountIdHex && event.data[1].toHex() === dest && event.data[2].toString() === String(amount)), 'transfer event must match the submitted fields');
        return { number: next, hash: hash.toHex(), extrinsicIndex: index, accountId: account.accountIdHex, destination: dest, amount: String(amount), nonce: nonce.toString() };
      }
      next++;
    }
    await delay(1000);
  }
  throw new Error('timed out waiting for finalized transfer');
}

try {
  assert.equal((await api.rpc.system.chain()).toString(), 'Development');
  assert.equal(api.runtimeVersion.specVersion.toNumber(), 119);
  assert.equal(api.runtimeVersion.transactionVersion.toNumber(), 7);
  assert(!api.runtimeMetadata.asLatest.apis.some(({ name }) => name.toString() === 'Benchmark'), 'non-benchmark runtime required');
  await transfer(alice, fixtureAccount.accountIdHex, '1000000000000000');
  const result = await transfer(fixtureAccount, destination.accountIdHex, '10000000000000');
  console.log(JSON.stringify(result, null, 2));
} finally {
  await api.disconnect();
}
