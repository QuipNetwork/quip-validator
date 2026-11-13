// @ts-nocheck
// On-chain smoke test for the pallet-xqvm / XQuad integration.
//
// Replays XQuad conformance vectors through the running node and asserts the
// chain reproduces the toolchain's normative expectations exactly: the same
// output values, and -- since 0.4.0 meters work rather than instructions --
// the same step count. A divergence here is a consensus-relevant divergence
// between the pallet's embedded VM and the reference implementation.
//
// Prerequisites:
//   make wasm-signer && npm run build --prefix js/quip-signer
//   cargo build -p quip-network-node
//   target/debug/quip-network-node --dev
//   vectors assembled by the pinned xqasm library into VECTORS_DIR
//
// `scripts/test-xqvm-onchain.sh` does all of this, including the node.
// To run this file on its own against a node that is already up:
//   QUIP_WS_URL=ws://127.0.0.1:9944 VECTORS_DIR=... node <this file>

import { ApiPromise, WsProvider } from '@polkadot/api';
import { GenericExtrinsicSignatureV4 } from '@polkadot/types';
import { u8aToHex } from '@polkadot/util';
import { addressEq, blake2AsHex } from '@polkadot/util-crypto';
import { readFileSync, readdirSync } from 'node:fs';
import { dirname, join, resolve } from 'node:path';
import { fileURLToPath } from 'node:url';

// This file lives at js/quip-signer/test/, three levels below the root.
const REPO_ROOT = process.env.QUIP_REPO || resolve(dirname(fileURLToPath(import.meta.url)), '../../..');
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
/// extrinsic plus whether it succeeded. Pass an explicit nonce to submit
/// several extrinsics without waiting for each one; the default, -1, asks
/// the node for the next nonce and so needs the previous one included.
async function submit (api, tx, account, signer, nonce = -1) {
  await tx.signAsync(account.address, { nonce, signer });

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

  // These two pinned 0.5.1 fixtures use a 64 GiB reference budget.
  // Replay every other vector normally, including future large-budget ones:
  // an unexpected memory failure must fail this gate, never become a skip.
  const memoryLimitRejections = new Set([
    'xqmx-grid/allocator_size_at_the_maximum',
    'xqmx-grid/model_growth_onto_the_maximum'
  ]);
  const seenMemoryLimitRejections = new Set();
  let replayed = 0;
  let memoryRejected = 0;
  check(names.length === 63, 'pinned xqvm 0.5.1 corpus has 63 step-asserting vectors');
  check(maxVmMemory === 16n * 1024n * 1024n, 'replay memory cap is pinned at 16 MiB');

  const vectors = names.map((name) => {
    const bytecode = readFileSync(join(VECTORS_DIR, `${name}.xqb`));
    const expected = JSON.parse(readFileSync(join(VECTORS_DIR, `${name}.expected.json`), 'utf8'));

    return {
      vectorPath: readFileSync(join(VECTORS_DIR, `${name}.path`), 'utf8').trim(),
      bytecode,
      expected,
      inputs: JSON.parse(readFileSync(join(VECTORS_DIR, `${name}.inputs.json`), 'utf8')),
      hash: blake2AsHex(bytecode, 256),
      stepLimit: BigInt(expected.steps) + 100n
    };
  });

  // Waiting for each extrinsic before signing the next costs one block per
  // extrinsic, two per vector. Sign every store_program and then every
  // execute with ascending nonces and submit them together: the pool orders
  // them by nonce, so each execute applies after every store, and each
  // result is still matched to its own extrinsic.
  const firstNonce = (await api.rpc.system.accountNextIndex(alice.address)).toNumber();
  const storeResults = vectors.map(({ bytecode }, i) => submit(
    api,
    api.tx.xqvm.storeProgram(u8aToHex(bytecode)),
    alice,
    signer,
    firstNonce + i
  ));
  const executeResults = vectors.map(({ hash, inputs, stepLimit }, i) => submit(
    api,
    api.tx.xqvm.execute(hash, inputs.calldata, inputs.output_slots, stepLimit),
    alice,
    signer,
    firstNonce + vectors.length + i
  ));
  const results = await Promise.all([...storeResults, ...executeResults]);
  const storedAll = results.slice(0, vectors.length);
  const executedAll = results.slice(vectors.length);

  for (const [i, { vectorPath, bytecode, expected, inputs, hash }] of vectors.entries()) {
    const expectsMemoryLimit = memoryLimitRejections.has(vectorPath);
    console.log(`== vector ${vectorPath} ==`);

    // ---- store_program
    const stored = storedAll[i];

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
          addressEq(event.data[1].toString(), alice.address),
          'ProgramStored names the submitting account as owner'
        );
        check(
          event.data[2].toNumber() === bytecode.length,
          `stored size is ${bytecode.length} bytes`
        );
      }
    }

    // A byte-for-byte comparison, not a presence check: the wire format
    // drifting between the assembler and the pallet is exactly what this
    // suite exists to catch.
    const onChain = await api.query.xqvm.programs(hash);
    check(
      onChain.isSome && u8aToHex(onChain.unwrap()) === u8aToHex(bytecode),
      'stored bytes read back from chain storage byte for byte'
    );

    // ---- execute
    const executed = executedAll[i];

    if (expectsMemoryLimit) {
      seenMemoryLimitRejections.add(vectorPath);
      check(inputs.memory_limit === 68_719_476_736, `${vectorPath} pins a 64 GiB reference budget`);
      const reason = executed.ok ? 'success' : describeFailure(api, executed.failed.event ?? executed.failed);
      const rejected = !executed.ok && reason === 'xqvm.VmMemoryLimitExceeded';
      check(rejected, `${vectorPath}: expected xqvm.VmMemoryLimitExceeded, got ${reason}`);
      check(
        !executed.events.some((e) => api.events.xqvm.ProgramExecuted.is(e)),
        'memory-cap rejection emitted no ProgramExecuted event'
      );
      if (rejected) memoryRejected += 1;
      continue;
    }

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

    // The 0.5.0 vector format omits trailing Unset slots and spells
    // interior Unset slots as null. The pallet preserves every requested
    // slot and represents Unset as zero. Compare the full event width.
    check(expected.outputs.length <= inputs.output_slots, 'vector outputs fit the requested slots');
    const expectedOutputs = Array.from(
      { length: inputs.output_slots },
      (_, i) => expected.outputs[i] ?? 0
    );

    check(
      JSON.stringify(outputs) === JSON.stringify(expectedOutputs),
      `outputs ${JSON.stringify(outputs)} match the vector with unwritten slots zero-filled`
    );
    check(
      stepsUsed === BigInt(expected.steps),
      `steps_used ${stepsUsed} matches the vector's ${expected.steps}`
    );
    replayed += 1;
  }

  check(
    replayed === 61 && memoryRejected === 2 && seenMemoryLimitRejections.size === 2,
    'pinned coverage: 61 successful replays and both named memory-cap rejections'
  );
  console.log(`== ${replayed} replayed, ${memoryRejected} rejected (pallet MaxVmMemory) ==`);

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
