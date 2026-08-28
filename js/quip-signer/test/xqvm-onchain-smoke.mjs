// @ts-nocheck
// On-chain smoke test for the pallet-xqvm / XQuad 0.4.0-rc1 integration.
//
// Replays XQuad conformance vectors through the running node and asserts the
// chain reproduces the toolchain's normative expectations exactly: the same
// output values, and -- since 0.4.0 meters work rather than instructions --
// the same step count. A divergence here is a consensus-relevant divergence
// between the pallet's embedded VM and the reference implementation.
//
// Prerequisites:
//   cargo build --release -p quip-network-node
//   target/release/quip-network-node --dev
//   vectors assembled with `xquad asm` into VECTORS_DIR
//
// Run from the repository root:
//   QUIP_WS_URL=ws://127.0.0.1:9944 VECTORS_DIR=... node <this file>

import { ApiPromise, WsProvider } from '@polkadot/api';
import { GenericExtrinsicSignatureV4 } from '@polkadot/types';
import { u8aToHex } from '@polkadot/util';
import { blake2AsHex } from '@polkadot/util-crypto';
import { readFileSync, readdirSync } from 'node:fs';
import { join } from 'node:path';

const REPO_ROOT = process.env.QUIP_REPO || '/Users/konrad/Quip/quip-validator';
const { DevSeedProvider, patchExtrinsicSignFake, QuipSigner } = await import(
  `${REPO_ROOT}/js/quip-signer/dist/index.js`
);

const WS_URL = process.env.QUIP_WS_URL || 'ws://127.0.0.1:9944';
const VECTORS_DIR = process.env.VECTORS_DIR;
const REPO = REPO_ROOT;

const ALICE = {
  name: 'Quip Alice',
  seedHex: '0xe5be9a5092b81bca64be81d212e7f2f9eba183bb7a90954f7b76361f6edb5c0a'
};

let failures = 0;
let checks = 0;

function check (condition, message) {
  checks += 1;
  if (condition) {
    console.log(`    ok   ${message}`);
  } else {
    failures += 1;
    console.log(`    FAIL ${message}`);
  }
}

/// Sign, submit, and wait for inclusion. Returns the events for this
/// extrinsic plus whether it succeeded.
async function submit (api, tx, account, signer) {
  await tx.signAsync(account.address, { nonce: -1, signer });

  const submittedHex = tx.toHex().toLowerCase();

  return new Promise((resolve, reject) => {
    let unsubscribe = () => undefined;
    let settled = false;

    api.rpc.author.submitAndWatchExtrinsic.raw(submittedHex, (rawStatus) => {
      const status = api.createType('ExtrinsicStatus', rawStatus);

      if (status.isInvalid || status.isDropped || status.isUsurped || status.isFinalityTimeout) {
        unsubscribe();
        reject(new Error(`submission failed with ${status.type}`));
      } else if (status.isInBlock && !settled) {
        settled = true;
        const blockHash = status.asInBlock;

        void (async () => {
          // The raw RPC result is inspected as hex: the runtime authors bare
          // V5 inherents that polkadot-js 16 cannot decode as V4 extrinsics.
          const rawBlock = await api.rpc.chain.getBlock.raw(blockHash);
          const index = rawBlock.block.extrinsics.findIndex(
            (encoded) => typeof encoded === 'string' && encoded.toLowerCase() === submittedHex
          );

          if (index === -1) {
            throw new Error('submitted extrinsic was not found in the block');
          }

          const blockApi = await api.at(blockHash);
          const all = await blockApi.query.system.events();
          const mine = all.filter(
            ({ phase }) => phase.isApplyExtrinsic && phase.asApplyExtrinsic.toNumber() === index
          );

          const ok = mine.some(({ event }) => api.events.system.ExtrinsicSuccess.is(event));
          const failed = mine.find(({ event }) => api.events.system.ExtrinsicFailed.is(event));

          return { ok, failed, events: mine.map(({ event }) => event), blockHash };
        })()
          .then((result) => {
            unsubscribe();
            resolve(result);
          })
          .catch((error) => {
            unsubscribe();
            reject(error);
          });
      }
    })
      .then((unsub) => { unsubscribe = unsub; })
      .catch(reject);
  });
}

function describeFailure (api, failedEvent) {
  const [dispatchError] = failedEvent.data;

  if (dispatchError.isModule) {
    const meta = api.registry.findMetaError(dispatchError.asModule);

    return `${meta.section}.${meta.name}`;
  }

  return dispatchError.toString();
}

// ---------------------------------------------------------------- setup ----

const wasmBytes = readFileSync(
  join(REPO, 'js/quip-transaction-crypto-wasm/quip_transaction_crypto_wasm_bg.wasm')
);
const wasmModule = await import(
  join(REPO, 'js/quip-transaction-crypto-wasm/quip_transaction_crypto_wasm.js')
);

await wasmModule.default({ module_or_path: wasmBytes });
patchExtrinsicSignFake(GenericExtrinsicSignatureV4);

const { accounts, provider } = await DevSeedProvider.fromSeeds(wasmModule, [ALICE]);
const [alice] = accounts;
const signer = new QuipSigner(provider);

const api = await ApiPromise.create({ provider: new WsProvider(WS_URL) });

try {
  // ------------------------------------------------------- node liveness ----
  console.log('== node liveness ==');
  const runtime = api.runtimeVersion;
  console.log(`    chain: ${await api.rpc.system.chain()}  runtime: ${runtime.specName}/${runtime.specVersion}`);

  const startHead = await api.rpc.chain.getHeader();
  const start = startHead.number.toNumber();

  const advanced = await new Promise((resolve, reject) => {
    const timer = setTimeout(() => reject(new Error('no new block within 45s')), 45_000);

    api.rpc.chain.subscribeNewHeads((header) => {
      if (header.number.toNumber() > start) {
        clearTimeout(timer);
        resolve(header.number.toNumber());
      }
    }).then((unsub) => setTimeout(unsub, 46_000)).catch(reject);
  });

  check(advanced > start, `chain authored a new block (#${start} -> #${advanced})`);

  // ------------------------------------------- runtime constants exposed ----
  console.log('== pallet constants ==');
  const maxStepLimit = api.consts.xqvm.maxStepLimit.toBigInt();
  const maxProgramSize = api.consts.xqvm.maxProgramSize.toNumber();
  const maxVmMemory = api.consts.xqvm.maxVmMemory.toBigInt();

  console.log(
    `    MaxStepLimit=${maxStepLimit}  MaxProgramSize=${maxProgramSize}  MaxVmMemory=${maxVmMemory}`
  );
  check(maxStepLimit > 0n, 'MaxStepLimit is derived and non-zero');
  check(
    maxVmMemory > 0n && maxVmMemory <= 32n * 1024n * 1024n,
    `MaxVmMemory is set and under the runtime heap (${maxVmMemory} bytes)`
  );

  // ------------------------------------------------ conformance replays ----
  const names = readdirSync(VECTORS_DIR)
    .filter((f) => f.endsWith('.expected.json'))
    .map((f) => f.replace(/\.expected\.json$/u, ''))
    .sort();

  for (const name of names) {
    console.log(`== vector ${name} ==`);

    const bytecode = readFileSync(join(VECTORS_DIR, `${name}.xqb`));
    const expected = JSON.parse(readFileSync(join(VECTORS_DIR, `${name}.expected.json`), 'utf8'));
    const inputs = JSON.parse(readFileSync(join(VECTORS_DIR, `${name}.inputs.json`), 'utf8'));
    const hash = blake2AsHex(bytecode, 256);

    // ---- store_program
    const stored = await submit(
      api,
      api.tx.xqvm.storeProgram(u8aToHex(bytecode)),
      alice,
      signer
    );

    if (!stored.ok) {
      const reason = describeFailure(api, stored.failed.event ?? stored.failed);

      check(reason === 'xqvm.ProgramAlreadyExists', `store_program: ${reason}`);
    } else {
      const event = stored.events.find((e) => api.events.xqvm.ProgramStored.is(e));

      check(Boolean(event), 'store_program emitted ProgramStored');
      if (event) {
        check(
          event.data[0].toHex() === hash,
          `stored hash matches blake2-256 of the bytecode (${hash.slice(0, 18)}...)`
        );
        check(
          event.data[2].toNumber() === bytecode.length,
          `stored size is ${bytecode.length} bytes`
        );
      }
    }

    const onChain = await api.query.xqvm.programs(hash);
    check(onChain.isSome, 'program is readable from chain storage');

    // ---- execute
    const stepLimit = BigInt(expected.steps) + 100n;
    const executed = await submit(
      api,
      api.tx.xqvm.execute(hash, inputs.calldata, inputs.output_slots, stepLimit),
      alice,
      signer
    );

    if (!executed.ok) {
      check(false, `execute failed: ${describeFailure(api, executed.failed.event ?? executed.failed)}`);
      continue;
    }

    const event = executed.events.find((e) => api.events.xqvm.ProgramExecuted.is(e));

    if (!event) {
      check(false, 'execute emitted ProgramExecuted');
      continue;
    }

    const outputs = event.data[3].toJSON().map((v) => Number(v));
    const stepsUsed = event.data[2].toBigInt();

    check(
      JSON.stringify(outputs) === JSON.stringify(expected.outputs),
      `outputs ${JSON.stringify(outputs)} match the vector`
    );
    check(
      stepsUsed === BigInt(expected.steps),
      `steps_used ${stepsUsed} matches the vector's ${expected.steps}`
    );
  }

  // -------------------------------------------------- negative controls ----
  console.log('== negative controls ==');

  const runaway = readFileSync(join(VECTORS_DIR, 'runaway.xqb'));
  const runawayHash = blake2AsHex(runaway, 256);

  const storedRunaway = await submit(api, api.tx.xqvm.storeProgram(u8aToHex(runaway)), alice, signer);

  check(
    storedRunaway.ok ||
      describeFailure(api, storedRunaway.failed.event ?? storedRunaway.failed) ===
        'xqvm.ProgramAlreadyExists',
    'non-halting program stores (it is statically valid)'
  );

  const bounded = await submit(
    api,
    api.tx.xqvm.execute(runawayHash, [], 0, 500n),
    alice,
    signer
  );

  check(
    !bounded.ok &&
      describeFailure(api, bounded.failed.event ?? bounded.failed) === 'xqvm.VmStepLimitExceeded',
    'a non-halting program is stopped by the step limit, not by the block'
  );

  const zero = await submit(api, api.tx.xqvm.execute(runawayHash, [], 0, 0n), alice, signer);

  check(
    !zero.ok && describeFailure(api, zero.failed.event ?? zero.failed) === 'xqvm.ZeroStepLimit',
    'a zero step limit is rejected up front'
  );

  const overLimit = await submit(
    api,
    api.tx.xqvm.execute(runawayHash, [], 0, maxStepLimit + 1n),
    alice,
    signer
  );

  check(
    !overLimit.ok &&
      describeFailure(api, overLimit.failed.event ?? overLimit.failed) === 'xqvm.StepLimitTooHigh',
    'a step limit above MaxStepLimit is rejected'
  );

  // The allocation budget (QUI-1012): a program that asks for more memory
  // than MaxVmMemory must be refused by the budget, not by the step limit
  // and not by a runtime trap. Before the budget was set this program ran
  // against xqvm's 1 GiB off-chain default.
  const overBudget = readFileSync(join(VECTORS_DIR, 'alloc_over_budget.xqb'));
  const overBudgetHash = blake2AsHex(overBudget, 256);

  const storedOverBudget = await submit(
    api,
    api.tx.xqvm.storeProgram(u8aToHex(overBudget)),
    alice,
    signer
  );

  check(
    storedOverBudget.ok ||
      describeFailure(api, storedOverBudget.failed.event ?? storedOverBudget.failed) ===
        'xqvm.ProgramAlreadyExists',
    'an over-allocating program stores (it is statically valid)'
  );

  // ~2.1M steps of headroom: the step bound must not be what stops this.
  const allocRun = await submit(
    api,
    api.tx.xqvm.execute(overBudgetHash, [], 0, 3_000_000n),
    alice,
    signer
  );

  check(
    !allocRun.ok &&
      describeFailure(api, allocRun.failed.event ?? allocRun.failed) ===
        'xqvm.VmMemoryLimitExceeded',
    'an allocation above MaxVmMemory is refused by the budget'
  );

  // The chain must still be authoring after all of that.
  const endHead = await api.rpc.chain.getHeader();

  check(endHead.number.toNumber() > start, `chain still authoring at #${endHead.number.toNumber()}`);
} finally {
  await api.disconnect();
}

console.log(`\n${checks - failures}/${checks} checks passed`);
process.exit(failures === 0 ? 0 : 1);
