// Golden parity for the browser signer's wasm build (WP2 batch 1, design §6),
// through the real wasm exports rather than mocks:
// - the `signer` cases of the QUIP HD parity file that pqhybridsign, the SDK
//   fork, transaction-crypto-core and the Python signer also check;
// - the H4 golden vectors that transaction-crypto-core and the Python signer
//   are gated on.
// Needs the wasm module built first (`make wasm-signer`), as CI does.
import assert from 'node:assert/strict';
import { createHash } from 'node:crypto';
import { readFileSync } from 'node:fs';
import initWasm, * as wasm from '../../quip-transaction-crypto-wasm/quip_transaction_crypto_wasm.js';

const repo = new URL('../../../', import.meta.url);
const read = (path) => readFileSync(new URL(path, repo));

await initWasm({
  module_or_path: read('js/quip-transaction-crypto-wasm/quip_transaction_crypto_wasm_bg.wasm')
});

const bare = (hex) => hex.replace(/^0x/u, '');
const sha256 = (hex) => createHash('sha256').update(Buffer.from(bare(hex), 'hex')).digest('hex');

// QUIP HD parity file, signer layer. The signers derive H4 only.
const parity = JSON.parse(read('crates/transaction-crypto-core/tests/vectors/quip_hd_parity.json'));
let positives = 0;
for (const testCase of parity.positive) {
  if (!testCase.scope.includes('signer')) continue;
  const key = testCase.keys.H4;
  const seed = wasm.seedFromMnemonic(testCase.suri);
  assert.equal(bare(seed), key.seed, testCase.name);
  const publicKey = wasm.publicFromSeed(seed);
  assert.equal(sha256(publicKey), key.public_sha256, testCase.name);
  if (key.public) assert.equal(bare(publicKey), key.public, testCase.name);
  positives += 1;
}
assert(positives >= 9, `${positives} signer positives`);

let negatives = 0;
for (const testCase of parity.negative) {
  if (!testCase.rejected_by.includes('signer')) continue;
  assert.throws(() => wasm.seedFromMnemonic(testCase.suri), undefined, testCase.name);
  negatives += 1;
}
assert(negatives >= 26, `${negatives} signer negatives`);

// Positives only the SDK accepts (junctions, the dev phrase) are refused here.
let sdkOnly = 0;
for (const testCase of parity.positive) {
  if (testCase.scope.includes('signer') || !testCase.scope.includes('sdk')) continue;
  assert.throws(() => wasm.seedFromMnemonic(testCase.suri), undefined, testCase.name);
  sdkOnly += 1;
}
assert(sdkOnly >= 15, `${sdkOnly} sdk-only positives`);

// H4 golden vectors (crates/transaction-crypto-core/tests/golden_vectors.txt).
const vectors = Object.fromEntries(
  read('crates/transaction-crypto-core/tests/golden_vectors.txt')
    .toString()
    .split('\n')
    .filter((line) => line.trim())
    .map((line) => line.trim().split('=', 2))
);
const TEST_PHRASE = 'bottom drive obey lake curtain smoke basket hold race lonely fit walk';
const seeds = {
  seed_01: '01'.repeat(32),
  seed_07: '07'.repeat(32),
  seed_09: '09'.repeat(32),
  seed_11: '0b'.repeat(32), // [11u8; 32]
  bip39: bare(wasm.seedFromMnemonic(TEST_PHRASE)),
  bip39_pw: bare(wasm.seedFromMnemonic(`${TEST_PHRASE}///hunter2`))
};
for (const [name, seed] of Object.entries(seeds)) {
  assert.equal(seed, vectors[`${name}_seed`], `${name}: seed`);
  assert.equal(bare(wasm.publicFromSeed(`0x${seed}`)), vectors[`${name}_public`], `${name}: public`);
}
const messages = {
  msg_quip: Buffer.from('quip-message'),
  msg_empty: Buffer.alloc(0),
  msg_fixture: Buffer.from('quip-signer-fixture')
};
for (const name of ['seed_07', 'bip39']) {
  for (const [messageName, message] of Object.entries(messages)) {
    const envelope = wasm.signPayloadFromSeed(`0x${seeds[name]}`, `0x${message.toString('hex')}`);
    assert.equal(bare(envelope), vectors[`${name}_${messageName}_envelope`], `${name}/${messageName}`);
  }
}

console.log(`golden parity passed (${positives} signer positives, ${negatives} negatives, ${sdkOnly} sdk-only, ${Object.keys(seeds).length} golden seeds)`);
