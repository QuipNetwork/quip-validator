import type { SignerPayloadRaw } from '@polkadot/types/types';

import { readFileSync } from 'node:fs';

import { GenericExtrinsicSignatureV4 } from '@polkadot/types';
import { hexToU8a, u8aToHex } from '@polkadot/util';
import { blake2AsU8a, encodeAddress } from '@polkadot/util-crypto';

import {
  DevSeedProvider,
  messageToSign,
  patchExtrinsicSignFake,
  QuipSigner,
  QUIP_ENVELOPE_LEN,
  QUIP_PUBLIC_KEY_LEN
} from '../src/index.js';

function assert(condition: unknown, message: string): asserts condition {
  if (!condition) {
    throw new Error(message);
  }
}

async function rejects(action: () => Promise<unknown>, pattern: RegExp): Promise<void> {
  try {
    await action();
  } catch (error) {
    assert(pattern.test((error as Error).message), `unexpected error: ${(error as Error).message}`);

    return;
  }

  throw new Error(`expected rejection matching ${pattern}`);
}

function bytesHex(length: number, value: number): string {
  return u8aToHex(new Uint8Array(length).fill(value));
}

function raw(address: string, data: string): SignerPayloadRaw {
  return { address, data, type: 'bytes' };
}

interface SigningFixture {
  accountIdHex: string;
  actualMessageToSignHex: string;
  envelopeLength: number;
  payloadBoundaryCases: Array<{
    length: number;
    messageToSignHex: string;
    rawPayloadHex: string;
    signatureEnvelopeHex: string;
  }>;
  publicKeyHex: string;
  rawSigningPayloadHex: string;
  signatureEnvelopeHex: string;
  signedExtrinsicHex: string;
  ss58Address: string;
}

const fixture = JSON.parse(readFileSync(
  new URL('../../../../docs/polkadotjs/fixtures/hybrid-signing.json', import.meta.url),
  'utf8'
)) as SigningFixture;

assert(fixture.envelopeLength === QUIP_ENVELOPE_LEN, 'Rust/TypeScript envelope sizes differ');
assert(
  messageToSign(fixture.rawSigningPayloadHex) === fixture.actualMessageToSignHex,
  'representative Rust signing message differs from TypeScript'
);

for (const fixtureCase of fixture.payloadBoundaryCases) {
  assert(
    messageToSign(fixtureCase.rawPayloadHex) === fixtureCase.messageToSignHex,
    `${fixtureCase.length}-byte Rust/TypeScript fixture drifted`
  );
  assert(
    hexToU8a(fixtureCase.signatureEnvelopeHex).length === QUIP_ENVELOPE_LEN,
    `${fixtureCase.length}-byte fixture envelope has the wrong size`
  );
}

const signedExtrinsic = hexToU8a(fixture.signedExtrinsicHex);
const compactMode = signedExtrinsic[0] & 0b11;
const compactPrefixLength = compactMode === 0
  ? 1
  : compactMode === 1
    ? 2
    : compactMode === 2
      ? 4
      : 5 + (signedExtrinsic[0] >> 2);
const signedBody = signedExtrinsic.subarray(compactPrefixLength);
const signatureOffset = 1 + 1 + 32;
const fixtureEnvelope = hexToU8a(fixture.signatureEnvelopeHex);

assert(signedBody[0] === 0x84, 'fixture extrinsic must be signed version 4');
assert(signedBody[1] === 0, 'fixture signer must use MultiAddress::Id');
assert(
  u8aToHex(signedBody.subarray(2, 34)) === fixture.accountIdHex,
  'extrinsic signer does not match the fixture account'
);
assert(
  u8aToHex(signedBody.subarray(signatureOffset, signatureOffset + QUIP_ENVELOPE_LEN)) ===
    fixture.signatureEnvelopeHex,
  'signed extrinsic does not contain the raw hybrid envelope'
);
assert(
  u8aToHex(fixtureEnvelope.subarray(0, QUIP_PUBLIC_KEY_LEN)) === fixture.publicKeyHex,
  'envelope public key differs from the fixture identity'
);

const accountId = new Uint8Array(32).fill(0x42);
const address = encodeAddress(accountId, 42);
const alternatePrefixAddress = encodeAddress(accountId, 2);
const envelope = bytesHex(QUIP_ENVELOPE_LEN, 0xab);

for (const length of [255, 256, 257]) {
  const payload = bytesHex(length, length & 0xff);
  const expected = length > 256
    ? u8aToHex(blake2AsU8a(hexToU8a(payload), 256))
    : payload;

  assert(messageToSign(payload) === expected, `${length}-byte payload rule drifted`);
}

const messages: string[] = [];
const signer = new QuipSigner({
  signPayload: async (_address, payloadHex) => {
    messages.push(payloadHex);

    return envelope;
  }
});
const firstPayload = bytesHex(257, 7);
const secondPayload = bytesHex(255, 8);
const [firstResult, secondResult] = await Promise.all([
  signer.signRaw(raw(address, firstPayload)),
  signer.signRaw(raw(address, secondPayload))
]);

assert(firstResult.id < secondResult.id, 'signer result ids must follow request order');
assert(messages[0] === messageToSign(firstPayload), 'long payload was not hashed before signing');
assert(messages[1] === secondPayload, 'short payload was not signed verbatim');
assert(hexToU8a(firstResult.signature).length === QUIP_ENVELOPE_LEN, 'envelope length drifted');
assert(firstResult.signature === envelope, 'signer must return the raw envelope without a variant byte');

const publicHex = bytesHex(QUIP_PUBLIC_KEY_LEN, 0x11);
const accountIdHex = u8aToHex(accountId);
const seedHex = bytesHex(32, 0x07);
let verifyResult = true;
const wasm = {
  accountIdFromPublic: async () => accountIdHex,
  publicFromSeed: async () => publicHex,
  signPayloadFromSeed: async () => envelope,
  verifyEnvelope: async (
    payloadHex: string,
    envelopeHex: string,
    actualAccountIdHex: string
  ) => verifyResult &&
    payloadHex === secondPayload &&
    envelopeHex === envelope &&
    actualAccountIdHex === accountIdHex
};
const { provider } = await DevSeedProvider.fromSeeds(wasm, [{ name: 'fixture', seedHex }]);

assert(provider.hasAccount(address), 'known account was not registered');
assert(provider.hasAccount(alternatePrefixAddress), 'seed lookup must be independent of SS58 prefix');
assert(await provider.signPayload(alternatePrefixAddress, secondPayload) === envelope, 'known account did not sign');

await rejects(
  () => provider.signPayload(encodeAddress(new Uint8Array(32).fill(0x24), 42), secondPayload),
  /No Quip seed registered/u
);
await rejects(() => provider.signPayload('not-an-address', secondPayload), /Malformed Quip SS58 address/u);

verifyResult = false;
await rejects(() => provider.signPayload(address, secondPayload), /invalid signature envelope/u);

const { provider: missingVerifyProvider } = await DevSeedProvider.fromSeeds(
  {
    accountIdFromPublic: async () => accountIdHex,
    publicFromSeed: async () => publicHex,
    signPayloadFromSeed: async () => envelope
  },
  [{ name: 'fixture', seedHex }]
);
await rejects(
  () => missingVerifyProvider.signPayload(address, secondPayload),
  /verifyEnvelope is not available/u
);

const proto = GenericExtrinsicSignatureV4.prototype as unknown as {
  signFake: (method: unknown, address: unknown, options: unknown) => unknown;
};
const upstreamResult = { upstream: true };

proto.signFake = () => upstreamResult;
patchExtrinsicSignFake(GenericExtrinsicSignatureV4);

function fakeSignatureContext(signatureLength: number): {
  _injectSignature: (...values: unknown[]) => unknown;
  createPayload: () => unknown;
  registry: {
    createType: () => { encodedLength: number };
    createTypeUnsafe: (type: string, params: unknown[]) => unknown;
  };
} {
  return {
    _injectSignature: (...values) => values,
    createPayload: () => 'payload',
    registry: {
      createType: () => ({ encodedLength: signatureLength }),
      createTypeUnsafe: (type, params) => ({ params, type })
    }
  };
}

const standardResult = proto.signFake.call(fakeSignatureContext(65), 'method', address, {});

assert(standardResult === upstreamResult, 'non-Quip registries must retain upstream signFake behavior');

const quipResult = proto.signFake.call(
  fakeSignatureContext(QUIP_ENVELOPE_LEN),
  'method',
  address,
  {}
) as Array<{ params?: unknown[] }>;
const fakeSignature = quipResult[1].params?.[0];

assert(fakeSignature instanceof Uint8Array, 'Quip signFake did not inject signature bytes');
assert(fakeSignature.length === QUIP_ENVELOPE_LEN, 'Quip fake signature has the wrong size');

interface GeneratedWasm {
  accountIdFromPublic: (publicHex: string) => string;
  default: (input: { module_or_path: Uint8Array }) => Promise<unknown>;
  publicFromSeed: (seedHex: string) => string;
  signPayloadFromSeed: (seedHex: string, payloadHex: string) => string;
  verifyEnvelope: (payloadHex: string, envelopeHex: string, accountIdHex: string) => boolean;
}

const generatedWasm = await import(
  new URL(
    '../../../quip-transaction-crypto-wasm/quip_transaction_crypto_wasm.js',
    import.meta.url
  ).href
) as GeneratedWasm;
const wasmBytes = readFileSync(new URL(
  '../../../quip-transaction-crypto-wasm/quip_transaction_crypto_wasm_bg.wasm',
  import.meta.url
));

await generatedWasm.default({ module_or_path: wasmBytes });

const wasmPublic = generatedWasm.publicFromSeed(
  (JSON.parse(readFileSync(
    new URL('../../../../docs/polkadotjs/fixtures/hybrid-signing.json', import.meta.url),
    'utf8'
  )) as { seedHex: string }).seedHex
);

assert(wasmPublic === fixture.publicKeyHex, 'generated WASM public key differs from Rust');
assert(
  generatedWasm.accountIdFromPublic(wasmPublic) === fixture.accountIdHex,
  'generated WASM account id differs from Rust'
);

for (const fixtureCase of fixture.payloadBoundaryCases) {
  const wasmEnvelope = generatedWasm.signPayloadFromSeed(
    (JSON.parse(readFileSync(
      new URL('../../../../docs/polkadotjs/fixtures/hybrid-signing.json', import.meta.url),
      'utf8'
    )) as { seedHex: string }).seedHex,
    fixtureCase.messageToSignHex
  );

  assert(
    wasmEnvelope === fixtureCase.signatureEnvelopeHex,
    `${fixtureCase.length}-byte generated WASM envelope differs from Rust`
  );
  assert(
    generatedWasm.verifyEnvelope(
      fixtureCase.messageToSignHex,
      wasmEnvelope,
      fixture.accountIdHex
    ),
    `${fixtureCase.length}-byte generated WASM envelope did not verify`
  );
}

console.log('quip-signer tests passed');
