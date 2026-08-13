// @ts-nocheck
// Local-node integration for Quip's injected signRaw protocol.
//
// Prerequisites:
//   make wasm-signer
//   npm run build --prefix js/quip-signer
//   cargo build -p quip-network-node
//   target/debug/quip-network-node --dev
//
// Run from the repository root with:
//   QUIP_WS_URL=ws://127.0.0.1:9944 npm run test:integration --prefix js/quip-signer

import { ApiPromise, WsProvider } from '@polkadot/api';
import { GenericExtrinsicSignatureV4 } from '@polkadot/types';
import { BN, hexToU8a, u8aToHex } from '@polkadot/util';
import { addressEq, encodeAddress } from '@polkadot/util-crypto';
import { readFileSync } from 'node:fs';

import {
  DevSeedProvider,
  patchExtrinsicSignFake,
  QuipSigner,
  QUIP_ENVELOPE_LEN
} from '../dist/index.js';
import initWasm, * as wasm from '../../quip-transaction-crypto-wasm/quip_transaction_crypto_wasm.js';

const WS_URL = process.env.QUIP_WS_URL || 'ws://127.0.0.1:9944';
const DEV_SEEDS = [
  {
    name: 'Quip Alice',
    seedHex: '0xe5be9a5092b81bca64be81d212e7f2f9eba183bb7a90954f7b76361f6edb5c0a'
  },
  {
    name: 'Quip Bob',
    seedHex: '0x398f0c28f98885e046333d4a41c19cee4c37368a9832c6502f6cfd182e2aef89'
  }
];

function assert (condition, message) {
  if (!condition) {
    throw new Error(message);
  }
}

async function submitAndInspect (api, tx, account, signer) {
  const payment = await tx.paymentInfo(account.address, { signer });

  assert(payment.partialFee.gtn(0), 'fee estimation returned a zero fee');

  await tx.signAsync(account.address, { nonce: -1, signer });

  assert(tx.isSigned, 'extrinsic signed bit was not set');
  assert(
    (tx.version & 0x7f) === 4 && (tx.version & 0x80) !== 0,
    `expected signed extrinsic version 4, got ${tx.version}`
  );
  assert(addressEq(tx.signer.toString(), account.address), 'encoded signer does not match account');

  const envelope = tx.signature.toU8a();

  assert(
    envelope.length === QUIP_ENVELOPE_LEN,
    `expected ${QUIP_ENVELOPE_LEN}-byte envelope, got ${envelope.length}`
  );

  const decoded = api.createType('Extrinsic', tx.toHex());

  assert(decoded.toHex() === tx.toHex(), 'signed extrinsic did not round-trip through the decoder');

  const submittedHex = tx.toHex().toLowerCase();

  await new Promise((resolve, reject) => {
    let unsubscribe = () => undefined;
    let isCheckingBlock = false;

    api.rpc.author.submitAndWatchExtrinsic.raw(submittedHex, (rawStatus) => {
      const status = api.createType('ExtrinsicStatus', rawStatus);

      if (status.isInvalid || status.isDropped || status.isUsurped || status.isFinalityTimeout) {
        unsubscribe();
        reject(new Error(`transaction submission failed with ${status.type}`));
      } else if (status.isInBlock && !isCheckingBlock) {
        isCheckingBlock = true;

        const blockHash = status.asInBlock;

        void (async () => {
          // Do not call decoded chain_getBlock here. The workspace runtime can
          // author bare V5 inherents that polkadot-js 16 cannot decode as V4
          // Extrinsics. The raw RPC result is safe to inspect as JSON hex.
          const rawBlock = await api.rpc.chain.getBlock.raw(blockHash);
          const extrinsics = rawBlock?.block?.extrinsics;

          assert(Array.isArray(extrinsics), 'raw block did not contain an extrinsics array');

          const extrinsicIndex = extrinsics.findIndex((encoded) =>
            typeof encoded === 'string' && encoded.toLowerCase() === submittedHex
          );

          assert(extrinsicIndex !== -1, 'submitted extrinsic was not found in the raw block');

          const blockApi = await api.at(blockHash);
          const events = await blockApi.query.system.events();
          const succeeded = events.some(({ event, phase }) =>
            phase.isApplyExtrinsic &&
            phase.asApplyExtrinsic.toNumber() === extrinsicIndex &&
            api.events.system.ExtrinsicSuccess.is(event)
          );

          assert(succeeded, `extrinsic ${extrinsicIndex} was included without ExtrinsicSuccess`);
        })()
          .then(() => {
            unsubscribe();
            resolve();
          })
          .catch((error) => {
            unsubscribe();
            reject(error);
          });
      }
    })
      .then((unsub) => {
        unsubscribe = unsub;
      })
      .catch(reject);
  });
}

function compactPrefixLength (encoded) {
  const mode = encoded[0] & 0b11;

  return mode === 0
    ? 1
    : mode === 1
      ? 2
      : mode === 2
        ? 4
        : 5 + (encoded[0] >> 2);
}

async function expectBadProof (api, tx, account, signer, mutate) {
  await tx.signAsync(account.address, { nonce: -1, signer });

  const encoded = tx.toU8a();

  mutate(encoded, compactPrefixLength(encoded));

  await api.rpc.author.submitExtrinsic(u8aToHex(encoded)).then(
    () => {
      throw new Error('invalid signature was accepted into the transaction pool');
    },
    (error) => {
      assert(
        /BadProof|Invalid Transaction|1010/u.test(error.message),
        `unexpected invalid-signature error: ${error.message}`
      );
    }
  );
}

const wasmBytes = readFileSync(new URL(
  '../../quip-transaction-crypto-wasm/quip_transaction_crypto_wasm_bg.wasm',
  import.meta.url
));

await initWasm({ module_or_path: wasmBytes });
patchExtrinsicSignFake(GenericExtrinsicSignatureV4);

const { accounts, provider } = await DevSeedProvider.fromSeeds(wasm, DEV_SEEDS);
const [alice, bob] = accounts;
const signer = new QuipSigner(provider);
const importedAlice = await provider.importMnemonic(
  'Imported funded Alice',
  DEV_SEEDS[0].seedHex
);

assert(addressEq(importedAlice.address, alice.address), 'mnemonic/seed import changed the account');
assert(
  provider.hasAccount(encodeAddress(alice.accountIdHex, 2)),
  'provider lookup depends on the SS58 prefix'
);

const api = await ApiPromise.create({ provider: new WsProvider(WS_URL) });

try {
  const aliceBefore = await api.query.system.account(alice.address);
  const bobBefore = await api.query.system.account(bob.address);

  await submitAndInspect(api, api.tx.system.remark('0x71756970'), alice, signer);
  await submitAndInspect(
    api,
    api.tx.balances.transferKeepAlive(bob.address, new BN(1)),
    alice,
    signer
  );
  await submitAndInspect(
    api,
    api.tx.balances.transferKeepAlive(alice.address, new BN(1)),
    bob,
    signer
  );
  await submitAndInspect(
    api,
    api.tx.system.remark(u8aToHex(new Uint8Array(512).fill(0x51))),
    importedAlice,
    signer
  );

  const aliceAfter = await api.query.system.account(alice.address);
  const bobAfter = await api.query.system.account(bob.address);

  assert(aliceAfter.nonce.gt(aliceBefore.nonce), 'Alice nonce did not increase');
  assert(bobAfter.nonce.gt(bobBefore.nonce), 'Bob nonce did not increase');

  await expectBadProof(
    api,
    api.tx.system.remark('0x74616d7065726564'),
    alice,
    signer,
    (encoded, prefixLength) => {
      const envelopeOffset = prefixLength + 1 + 1 + 32;

      encoded[envelopeOffset + 100] ^= 0xff;
    }
  );
  await expectBadProof(
    api,
    api.tx.system.remark('0x6d69736d617463686564'),
    alice,
    signer,
    (encoded, prefixLength) => {
      const addressOffset = prefixLength + 1 + 1;

      encoded.set(hexToU8a(bob.accountIdHex), addressOffset);
    }
  );

  const stale = api.tx.system.remark('0x7374616c65');

  await stale.signAsync(alice.address, {
    nonce: aliceAfter.nonce.subn(1),
    signer
  });
  await stale.send().then(
    () => {
      throw new Error('stale nonce was accepted into the transaction pool');
    },
    (error) => {
      assert(
        /Stale|Invalid Transaction|1010/u.test(error.message),
        `unexpected stale-nonce error: ${error.message}`
      );
    }
  );

  await new QuipSigner(provider)
    .signRaw({
      address: encodeAddress(new Uint8Array(32).fill(0x99), 42),
      data: '0x00',
      type: 'bytes'
    })
    .then(
      () => {
        throw new Error('unknown Quip account unexpectedly signed');
      },
      (error) => {
        assert(
          /No Quip seed registered/u.test(error.message),
          `unexpected unknown-account error: ${error.message}`
        );
      }
    );

  console.log('Quip local-node signing integration passed');
} finally {
  await api.disconnect();
}
