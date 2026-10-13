// Actual Polkadot REST API 0.3.2 HTTP conformance; metadata-only stock JS oracle.
import assert from 'node:assert/strict';
import { createHash } from 'node:crypto';
import { readFileSync, mkdirSync, writeFileSync } from 'node:fs';
import { join, resolve } from 'node:path';
import { Metadata, TypeRegistry } from '@polkadot/types';
import { packageInfo as typesPackage } from '@polkadot/types/packageInfo';
import { packageInfo as cryptoPackage } from '@polkadot/util-crypto/packageInfo';
import { blake2AsHex, decodeAddress } from '@polkadot/util-crypto';

assert.equal(typesPackage.version, '16.5.6');
assert.equal(cryptoPackage.version, '14.0.3');
const [directory, ...flags] = process.argv.slice(2);
assert(directory && flags.every((flag) => ['--expect-stock-truncation', '--expect-parse-truncation'].includes(flag)),
  'usage: node test/rest-conformance.mjs CAPTURE_DIRECTORY [--expect-stock-truncation | --expect-parse-truncation]');
const stock = flags.includes('--expect-stock-truncation');
const parseOnly = flags.includes('--expect-parse-truncation');
assert(!(stock && parseOnly), 'choose one known-failure configuration');
const base = new URL(process.env.QUIP_REST_URL || 'http://127.0.0.1:18015');
assert.equal(base.protocol, 'http:');
assert(['127.0.0.1', '[::1]'].includes(base.hostname), 'disposable localhost service only');
const evidence = process.env.QUIP_REST_EVIDENCE && resolve(process.env.QUIP_REST_EVIDENCE);
if (evidence) mkdirSync(evidence, { recursive: true });
const sha = (data) => createHash('sha256').update(data).digest('hex');
const hex = (data) => '0x' + Buffer.from(data).toString('hex');
const camel = (name) => name.replace(/_([a-z])/g, (_, c) => c.toUpperCase()).replace(/^./, (c) => c.toLowerCase());
const read = (name) => readFileSync(join(directory, name));
const manifestBytes = read('manifest.json');
const manifest = JSON.parse(manifestBytes);
assert.equal(manifest.schemaVersion, 1);
assert.equal(manifest.kind, 'local-non-benchmark');
assert.deepEqual(manifest.buildFeatures, ['std']);
assert(manifest.buildArtifact.sha256 && manifest.sourceCommit);
for (const [name, expected] of Object.entries(manifest.files)) {
  const data = read(name);
  assert.equal(data.length, expected.bytes, name);
  assert.equal(sha(data), expected.sha256, name);
}
assert.equal(manifest.blocks.length, 1, 'one freshly captured transfer block');
const block = manifest.blocks[0];
assert.equal(block.executionSpecVersion, 119);
assert.equal(block.postStateSpecVersion, 119);
assert.equal(block.transactionVersion, 7);
assert.equal(block.executionCodeSha256, manifest.buildArtifact.sha256);
assert.equal(block.metadataStateHash, block.parentHash);
for (const state of ['parent', 'post']) {
  const runtime = JSON.parse(read(`runtime-${state}-${block.number}.json`));
  assert.equal(runtime.specVersion, 119);
  assert.equal(runtime.transactionVersion, 7);
  const benchmarkId = blake2AsHex('Benchmark', 64);
  assert(!runtime.apis.some(([id]) => id === benchmarkId));
}
assert.deepEqual(manifest.files[block.metadataFile].params, [block.parentHash]);
assert.deepEqual(manifest.files[block.eventsFile].params.slice(1), [block.hash]);
assert(block.number <= manifest.finalizedAtCapture.number);
const registry = new TypeRegistry();
const metadata = new Metadata(registry, read(block.metadataFile));
assert.equal(metadata.version, 16);
registry.setMetadata(metadata); // No chain types, overrides, or custom registrations.
const rpc = JSON.parse(read(block.blockFile));
const header = registry.createType('Header', rpc.block.header);
assert.equal(header.hash.toHex(), block.hash);
assert.equal(header.parentHash.toHex(), block.parentHash);
assert.equal(header.number.toNumber(), block.number);
const records = registry.createType('Vec<EventRecord>', read(block.eventsFile));
assert.equal(hex(records.toU8a()), hex(read(block.eventsFile)));
assert.equal(rpc.block.extrinsics.length, 2);
assert.equal(block.extrinsics.length, 2);
assert.deepEqual(block.extrinsics.map((e) => [e.index, e.version, e.signed]), [[0, 5, false], [1, 132, true]]);
const responseHashes = {};
async function request(path, name, tx) {
  const options = { signal: AbortSignal.timeout(30000), redirect: 'error' };
  if (tx !== undefined) Object.assign(options, {
    method: 'POST', headers: { 'content-type': 'application/json' }, body: JSON.stringify({ tx })
  });
  const response = await fetch(new URL('/v1' + path, base), options);
  assert.equal(response.status, 200, path);
  const body = await response.text();
  responseHashes[name] = sha(body);
  if (evidence) writeFileSync(join(evidence, name + '.json'), body);
  return JSON.parse(body);
}
// REST renders integer fields as decimal strings. No address or byte-string
// normalization: those must match exactly. This fixture's numbers are safe JS integers.
function decimal(value) {
  if (typeof value === 'number') { assert(Number.isSafeInteger(value)); return String(value); }
  if (Array.isArray(value)) return value.map(decimal);
  if (value && typeof value === 'object') return Object.fromEntries(Object.entries(value).map(([k, v]) => [k, decimal(v)]));
  return value;
}
function storageData(value) {
  if (Array.isArray(value)) return value.map(storageData);
  if (value && typeof value === 'object') return Object.fromEntries(Object.entries(value).map(([k, v]) =>
    [k, (k === 'class' || k === 'paysFee') && typeof v === 'string' ? camel(v) : storageData(v)]));
  return value;
}
const expectedEvent = ({ event }) => ({ method: { pallet: event.section, method: event.method }, data: decimal(event.data.toPrimitive()) });
const version = await request('/version', 'version');
assert.equal(version.version, '0.3.2');
const result = await request(`/blocks/${block.hash}`, 'block'); // Default fee processing enabled.
assert(!result.partial && !result.decodeError, 'partial blocks are not conformance');
for (const key of ['hash', 'parentHash']) assert.equal(result[key], block[key]);
for (const key of ['stateRoot', 'extrinsicsRoot']) assert.equal(result[key], rpc.block.header[key]);
assert.equal(result.number, String(block.number));
assert.equal(result.finalized, true);
assert.equal(result.extrinsics.length, block.extrinsics.length);
const raw = await request(`/blocks/${block.hash}/extrinsics-raw`, 'raw');
assert.equal(BigInt(raw.number), BigInt(block.number));
assert.deepEqual(raw.extrinsics, rpc.block.extrinsics);
const events = await request(`/pallets/system/storage/events?at=${block.hash}`, 'events');
assert.deepEqual(events.at, { hash: block.hash, height: String(block.number) });
assert.equal(events.pallet, 'system');
assert.equal(events.storageItem, 'events');
assert.equal(events.value.length, records.length);
const expectedStorageEvents = records.map((record) => {
  const fields = record.event.meta.fields;
  assert(fields.every((field) => field.name.isSome), 'fixture events have named fields');
  const data = Object.fromEntries(fields.map((field, i) => [camel(field.name.unwrap().toString()), decimal(record.event.data[i].toPrimitive())]));
  return { phase: decimal(record.phase.toJSON()), event: { [record.event.section]: { [camel(record.event.method)]: storageData(data) } }, topics: record.topics.toJSON() };
});
assert.deepEqual(events.value, expectedStorageEvents, 'every storage event field, phase, index and order');
assert.deepEqual(result.onInitialize.events, records.filter((r) => r.phase.isInitialization).map(expectedEvent));
assert.deepEqual(result.onFinalize.events, records.filter((r) => r.phase.isFinalization).map(expectedEvent));
let truncations = 0;
let signatureBytes = 0;
let parseSignatureBytes = 0;
let parseTruncations = 0;
for (const expected of block.extrinsics) {
  const item = result.extrinsics[expected.index];
  assert(!item.decodeError);
  assert.equal(item.hash, expected.hash);
  assert.equal(blake2AsHex(rpc.block.extrinsics[expected.index]), expected.hash);
  assert.deepEqual(item.method, { pallet: camel(expected.pallet), method: camel(expected.call) });
  const perIndex = await request(`/blocks/${block.hash}/extrinsics/${expected.index}`, `extrinsic-${expected.index}`);
  assert.deepEqual(perIndex.at, { hash: block.hash, height: String(block.number) });
  assert.deepEqual(perIndex.extrinsics, item, 'per-index route retains all fields');
  // This route only decodes; it never submits or executes the transaction.
  const parsed = await request('/transaction/parse', `parse-${expected.index}`, rpc.block.extrinsics[expected.index]);
  assert.equal(parsed.isSigned, expected.signed);
  for (const key of ['method', 'args', 'hash', 'era']) assert.deepEqual(parsed[key], item[key], `parse/block ${key}`);
  for (const key of ['nonce', 'tip']) assert.equal(parsed[key] ?? null, item[key], `parse/block ${key}`);

  const scoped = records.filter((r) => r.phase.isApplyExtrinsic && r.phase.asApplyExtrinsic.toNumber() === expected.index);
  assert.deepEqual(item.events, scoped.map(expectedEvent), 'all phase-associated event fields');
  const outcomes = scoped.filter((r) => r.event.section === 'system' && ['ExtrinsicSuccess', 'ExtrinsicFailed'].includes(r.event.method));
  assert.equal(outcomes.length, 1);
  assert.equal(outcomes[0].event.method, 'ExtrinsicSuccess');
  assert.equal(item.success, true);
  assert.deepEqual(item.era, { immortalEra: '0x00' });
  if (!expected.signed) {
    assert.equal(item.signature, null);
    for (const key of ['signature', 'nonce', 'tip']) assert(!Object.hasOwn(parsed, key), `bare parse must omit ${key}`);
    assert.equal(item.nonce, null);
    assert.equal(item.tip, null);
    assert.equal(item.paysFee, false);
    assert.deepEqual(item.args, { now: expected.now });
  } else {
    const extrinsic = registry.createType('Extrinsic', rpc.block.extrinsics[expected.index]);
    const signature = extrinsic.signature.toU8a();
    assert.equal(signature.length, 1660);
    assert.equal(sha(signature), expected.envelopeSha256);
    assert.deepEqual(Object.keys(parsed.signature).sort(), ['signature', 'signer']);
    assert.deepEqual(parsed.signature.signer, item.signature.signer);
    parseSignatureBytes = (parsed.signature.signature.length - 2) / 2;
    if (stock || parseOnly) {
      assert.equal(parseSignatureBytes, 1659, 'known parse truncation length');
      assert.equal(parsed.signature.signature, hex(signature.subarray(1)), 'exact parse one-byte truncation must reproduce');
      parseTruncations++;
    } else {
      assert.equal(parseSignatureBytes, 1660, 'parse must retain the complete signature envelope');
      assert.equal(parsed.signature.signature, hex(signature), 'parse raw-envelope byte equality');
      assert.deepEqual(parsed.signature, item.signature, 'parse/block signature and signer equality');
    }

    assert.equal(hex(decodeAddress(item.signature.signer.id)), expected.accountId);
    assert.equal(hex(decodeAddress(item.args.dest.id)), expected.destination);
    assert.deepEqual(Object.keys(item.args).sort(), ['dest', 'value']);
    assert.equal(item.args.value, expected.amount);
    assert.equal(item.nonce, expected.nonce);
    assert.equal(item.tip, expected.tip);
    assert.equal(item.paysFee, true);
    const fee = scoped.find((r) => r.event.section === 'transactionPayment' && r.event.method === 'TransactionFeePaid');
    assert(fee);
    assert.equal(item.info.partialFee, fee.event.data[1].toString());
    assert.equal(item.info.kind, 'fromEvent');
    assert.deepEqual(item.info.weight, decimal(outcomes[0].event.data[0].toPrimitive()).weight);
    const transfer = scoped.filter((r) => r.event.section === 'balances' && r.event.method === 'Transfer');
    assert.equal(transfer.length, 1);
    assert.deepEqual(transfer[0].event.data.map((value, i) => i < 2 ? hex(decodeAddress(value.toString())) : value.toString()), [expected.accountId, expected.destination, expected.amount]);
    signatureBytes = (item.signature.signature.length - 2) / 2;
    if (stock) {
      assert.equal(item.signature.signature, hex(signature.subarray(1)), 'exact stock one-byte truncation must reproduce');
      truncations++;
    } else {
      assert.equal(signatureBytes, 1660, 'REST must retain the complete signature envelope');
      assert.equal(item.signature.signature, hex(signature), 'byte-for-byte envelope equality');
    }
  }
}
assert.equal(truncations, stock ? 1 : 0);
assert.equal(parseTruncations, stock || parseOnly ? 1 : 0);
const configuration = stock
  ? 'stock 0.3.2: known signature truncation reproduced (NOT CONFORMANT)'
  : parseOnly
    ? 'REST 0.3.2 with block patch only: known parse truncation reproduced (NOT CONFORMANT for parse)'
    : 'REST 0.3.2 tested block/event/parse routes: PASS';
const report = {
  configuration, oracle: { types: typesPackage.version, crypto: cryptoPackage.version },
  manifestSha256: sha(manifestBytes), sourceCommit: manifest.sourceCommit,
  runtimeCodeSha256: manifest.buildArtifact.sha256, blockHash: block.hash,
  blocks: 1, bare: 1, signed: 1, events: records.length, signatureBytes, parseSignatureBytes,
  knownTruncations: truncations, knownParseTruncations: parseTruncations, responseHashes
};
if (evidence) writeFileSync(join(evidence, 'report.json'), JSON.stringify(report, null, 2) + '\n');
console.log(JSON.stringify(report, null, 2));
