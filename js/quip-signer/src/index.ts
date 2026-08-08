import type { Signer, SignerResult } from '@polkadot/api/types';
import type { SignerPayloadRaw } from '@polkadot/types/types';
import type { HexString } from '@polkadot/util/types';

import { hexToU8a, u8aToHex } from '@polkadot/util';
import { blake2AsU8a, decodeAddress, encodeAddress } from '@polkadot/util-crypto';

type MaybePromise<T> = T | Promise<T>;
type Unsubscribe = () => void;

let nextSignerId = 0;
let signFakePatched = false;

interface SignFakeRegistry {
  createType: (type: string) => { encodedLength: number };
  createTypeUnsafe: (type: string, params: unknown[]) => unknown;
}

interface SignFakeProto {
  registry: SignFakeRegistry;
  createPayload: (method: unknown, options: unknown) => unknown;
  _injectSignature: (signer: unknown, signature: unknown, payload: unknown) => unknown;
  signFake: (method: unknown, address: unknown, options: unknown) => unknown;
}

export const QUIP_ACCOUNT_ID_LEN = 32;
export const QUIP_PUBLIC_KEY_LEN = 1344;
export const QUIP_SIGNATURE_LEN = 2484;
export const QUIP_ENVELOPE_LEN = QUIP_PUBLIC_KEY_LEN + QUIP_SIGNATURE_LEN;

function nextResultId (): number {
  nextSignerId = nextSignerId === Number.MAX_SAFE_INTEGER
    ? 1
    : nextSignerId + 1;

  return nextSignerId;
}

function decodeHex (name: string, value: string, expectedLength?: number): Uint8Array {
  if (!/^0x(?:[0-9a-f]{2})*$/iu.test(value)) {
    throw new Error(`${name} must be a 0x-prefixed, even-length hex string`);
  }

  const decoded = hexToU8a(value);

  if (expectedLength !== undefined && decoded.length !== expectedLength) {
    throw new Error(`${name} must decode to ${expectedLength} bytes (got ${decoded.length})`);
  }

  return decoded;
}

function accountIdFromAddress (address: string): string {
  if (!address || typeof address !== 'string') {
    throw new Error('Quip signer address must be a non-empty SS58 address');
  }

  let decoded: Uint8Array;

  try {
    decoded = decodeAddress(address);
  } catch (error) {
    throw new Error(`Malformed Quip SS58 address: ${(error as Error).message}`);
  }

  if (decoded.length !== QUIP_ACCOUNT_ID_LEN) {
    throw new Error(
      `Quip SS58 address must decode to ${QUIP_ACCOUNT_ID_LEN} account-id bytes (got ${decoded.length})`
    );
  }

  return u8aToHex(decoded).toLowerCase();
}

/**
 * Make polkadot-js fee estimation work with Quip's large hybrid signature.
 *
 * `tx.paymentInfo(address)` builds a dummy-signed extrinsic via
 * `GenericExtrinsicSignatureV4.signFake`, which upstream fills from a hardcoded
 * 256-byte `FAKE_SIGNATURE`. Quip's runtime signature is
 * `HybridTxSignature { public: [u8;1344], signature: [u8;2484] }` (3828 bytes),
 * so SCALE-decoding the 256-byte fake into that struct throws:
 *
 *   decodeU8aStruct: failed ... on public (index 1/2): [u8;1344]::
 *   Expected input with 1344 bytes, found 256 bytes
 *
 * This overrides `signFake` to size the fake signature from the registry's real
 * `ExtrinsicSignature` type, so fee estimation produces a correctly sized
 * extrinsic. It is idempotent and leaves the real signing path untouched.
 */
export function patchExtrinsicSignFake (
  signatureType: { prototype: unknown }
): void {
  if (signFakePatched) {
    return;
  }

  signFakePatched = true;

  const proto = signatureType.prototype as SignFakeProto;
  const upstreamSignFake = proto.signFake;

  proto.signFake = function (this: SignFakeProto, method, address, options) {
    if (!address) {
      throw new Error('Expected a valid address for signing');
    }

    let fakeLength: number;

    try {
      fakeLength = this.registry.createType('ExtrinsicSignature').encodedLength;
    } catch {
      return upstreamSignFake.call(this, method, address, options);
    }

    // This patch is installed by the opt-in Quip integration, but the Apps
    // registry can later reconnect to another chain in the same page. Preserve
    // upstream behavior unless metadata says the signature is exactly Quip's
    // fixed hybrid envelope.
    if (fakeLength !== QUIP_ENVELOPE_LEN) {
      return upstreamSignFake.call(this, method, address, options);
    }

    const payload = this.createPayload(method, options);
    const fake = new Uint8Array(fakeLength).fill(1);

    return this._injectSignature(
      this.registry.createTypeUnsafe('Address', [address]),
      this.registry.createTypeUnsafe('ExtrinsicSignature', [fake]),
      payload
    );
  };
}

export interface QuipWasmCrypto {
  accountIdFromPublic: (publicHex: string) => MaybePromise<string>;
  publicFromSeed: (seedHex: string) => MaybePromise<string>;
  seedFromMnemonic?: (secretUri: string) => MaybePromise<string>;
  signPayloadFromSeed: (seedHex: string, payloadHex: string) => MaybePromise<string>;
  verifyEnvelope?: (
    payloadHex: string,
    envelopeHex: string,
    accountIdHex: string
  ) => MaybePromise<boolean>;
}

export interface QuipSecretProvider {
  signPayload: (address: string, payloadHex: string) => Promise<string>;
}

export interface QuipInjectedAccount {
  address: string;
  genesisHash?: string | null;
  name?: string;
  type?: string;
}

export interface QuipSeedAccountInput {
  name: string;
  seedHex: string;
  genesisHash?: string | null;
}

export interface QuipMnemonicAccountInput {
  name: string;
  /**
   * An English BIP39 phrase, optionally followed by `///<password>`, or a
   * `0x`-prefixed 64-digit hex seed. Derivation junctions are not supported.
   */
  mnemonic: string;
  genesisHash?: string | null;
}

export interface QuipSeedAccount extends QuipInjectedAccount {
  accountIdHex: string;
  publicHex: string;
}

export interface QuipInjectedAccounts {
  get: () => Promise<QuipInjectedAccount[]>;
  subscribe: (cb: (accounts: QuipInjectedAccount[]) => void) => Unsubscribe;
}

export interface QuipInjectedExtension {
  accounts: QuipInjectedAccounts;
  signer: Signer;
}

export interface QuipInjectedProvider {
  version: string;
  enable: (origin: string) => Promise<QuipInjectedExtension>;
}

export interface QuipInjectionOptions {
  accounts: QuipInjectedAccount[];
  signer: Signer;
  version?: string;
}

interface InjectedWeb3Global {
  injectedWeb3?: Record<string, QuipInjectedProvider>;
}

// Substrate signs `SignedPayload::using_encoded`, which blake2-256-hashes the
// SCALE-encoded payload when it is longer than 256 bytes and otherwise signs it
// verbatim (see substrate `generic::SignedPayload`). The signer must reproduce
// this rule so the H3 signature is computed over the exact bytes the runtime
// verifies.
const MAX_UNHASHED_PAYLOAD_LEN = 256;

/**
 * Returns the exact message bytes (as hex) that the runtime signs for a payload.
 *
 * `data` is the SCALE-encoded `ExtrinsicPayload` produced by polkadot-js's
 * `SignerPayload.toRaw()` (method un-prefixed). Long payloads are blake2-256
 * hashed to match `SignedPayload::using_encoded`.
 */
export function messageToSign (dataHex: string): string {
  const data = decodeHex('raw signing payload', dataHex);

  return data.length > MAX_UNHASHED_PAYLOAD_LEN
    ? u8aToHex(blake2AsU8a(data, 256))
    : u8aToHex(data);
}

export class QuipSigner implements Signer {
  readonly #provider: QuipSecretProvider;

  public constructor(provider: QuipSecretProvider) {
    this.#provider = provider;
  }

  // Implemented as `signRaw` (not `signPayload`) on purpose: polkadot-js hands
  // `signRaw` the fully SCALE-encoded payload bytes via `toRaw()`, so the signer
  // needs no metadata-aware registry to reconstruct them. `SignerPayloadJSON`
  // has no raw-bytes field, so a `signPayload` implementation cannot sign
  // without rebuilding the payload from a registry.
  public async signRaw ({ address, data }: SignerPayloadRaw): Promise<SignerResult> {
    const id = nextResultId();
    accountIdFromAddress(address);
    decodeHex('raw signing payload', data);

    const signature = await this.#provider.signPayload(address, messageToSign(data));
    decodeHex('hybrid signature envelope', signature, QUIP_ENVELOPE_LEN);

    return {
      id,
      signature: signature as HexString
    };
  }
}

export class DevSeedProvider implements QuipSecretProvider {
  // Keyed by account-id hex (not the SS58 address) so lookups are independent
  // of the ss58 prefix the chain re-encodes signer addresses with.
  readonly #seedsByAccountId = new Map<string, string>();
  readonly #wasm: QuipWasmCrypto;
  readonly #ss58Format: number;

  private constructor(wasm: QuipWasmCrypto, ss58Format: number) {
    this.#wasm = wasm;
    this.#ss58Format = ss58Format;
  }

  public static async fromSeeds(
    wasm: QuipWasmCrypto,
    inputs: QuipSeedAccountInput[],
    ss58Format = 42
  ): Promise<{ accounts: QuipSeedAccount[]; provider: DevSeedProvider }> {
    const provider = new DevSeedProvider(wasm, ss58Format);
    const accounts = await Promise.all(
      inputs.map(({ genesisHash = null, name, seedHex }) =>
        provider.importSeed(name, seedHex, genesisHash)
      )
    );

    return { accounts, provider };
  }

  /**
   * Builds a provider from BIP39 mnemonics (or `0x` seed hex) by deriving each
   * master seed with the WASM module. The derivation matches substrate's
   * `Pair::from_phrase`, so imported accounts resolve to the same addresses the
   * runtime recognizes.
   */
  public static async fromMnemonics(
    wasm: QuipWasmCrypto,
    inputs: QuipMnemonicAccountInput[],
    ss58Format = 42
  ): Promise<{ accounts: QuipSeedAccount[]; provider: DevSeedProvider }> {
    const provider = new DevSeedProvider(wasm, ss58Format);
    const accounts = await Promise.all(
      inputs.map(({ genesisHash = null, mnemonic, name }) =>
        provider.importMnemonic(name, mnemonic, genesisHash)
      )
    );

    return { accounts, provider };
  }

  /** Registers a master seed and returns the derived Quip account. */
  public async importSeed (
    name: string,
    seedHex: string,
    genesisHash: string | null = null
  ): Promise<QuipSeedAccount> {
    decodeHex('seed', seedHex, 32);

    const publicHex = await this.#wasm.publicFromSeed(seedHex);
    decodeHex('H3 public key', publicHex, QUIP_PUBLIC_KEY_LEN);

    const accountIdHex = await this.#wasm.accountIdFromPublic(publicHex);
    const accountId = decodeHex('Quip account id', accountIdHex, QUIP_ACCOUNT_ID_LEN);
    const normalizedAccountId = u8aToHex(accountId).toLowerCase();
    const address = encodeAddress(accountId, this.#ss58Format);

    this.#seedsByAccountId.set(normalizedAccountId, seedHex);

    return { accountIdHex: normalizedAccountId, address, genesisHash, name, publicHex };
  }

  /**
   * Derives a master seed from a BIP39 phrase (or `0x` seed hex), registers it,
   * and returns the derived Quip account. Suitable for runtime account import.
   */
  public async importMnemonic (
    name: string,
    mnemonic: string,
    genesisHash: string | null = null
  ): Promise<QuipSeedAccount> {
    if (!this.#wasm.seedFromMnemonic) {
      throw new Error('seedFromMnemonic is not available in the supplied Quip WASM module');
    }

    const seedHex = await this.#wasm.seedFromMnemonic(mnemonic);

    return this.importSeed(name, seedHex, genesisHash);
  }

  public async signPayload(address: string, payloadHex: string): Promise<string> {
    const accountIdHex = accountIdFromAddress(address);
    decodeHex('message to sign', payloadHex);

    const seedHex = this.#seedsByAccountId.get(accountIdHex);

    if (!seedHex) {
      throw new Error(`No Quip seed registered for ${address}`);
    }

    if (!this.#wasm.verifyEnvelope) {
      throw new Error('verifyEnvelope is not available in the supplied Quip WASM module');
    }

    const envelopeHex = await this.#wasm.signPayloadFromSeed(seedHex, payloadHex);
    decodeHex('hybrid signature envelope', envelopeHex, QUIP_ENVELOPE_LEN);

    const isValid = await this.#wasm.verifyEnvelope(payloadHex, envelopeHex, accountIdHex);

    if (!isValid) {
      throw new Error(`Quip WASM returned an invalid signature envelope for ${address}`);
    }

    return envelopeHex;
  }

  public hasAccount(address: string): boolean {
    try {
      return this.#seedsByAccountId.has(accountIdFromAddress(address));
    } catch {
      return false;
    }
  }
}

export function createInjectedQuipProvider({
  accounts,
  signer,
  version = '0.1.0'
}: QuipInjectionOptions): QuipInjectedProvider {
  const injectedAccounts = accounts.map((account) => ({ ...account }));

  return {
    version,
    enable: async (_origin: string): Promise<QuipInjectedExtension> => ({
      accounts: {
        get: async () => injectedAccounts,
        subscribe: (cb): Unsubscribe => {
          cb(injectedAccounts);

          return () => undefined;
        }
      },
      signer
    })
  };
}

export function injectQuip(options: QuipInjectionOptions): void {
  const root = globalThis as typeof globalThis & InjectedWeb3Global;

  root.injectedWeb3 = {
    ...root.injectedWeb3,
    quip: createInjectedQuipProvider(options)
  };
}

export async function verifySignedPayload(
  wasm: QuipWasmCrypto,
  payloadHex: string,
  envelopeHex: string,
  accountIdHex: string
): Promise<boolean> {
  if (!wasm.verifyEnvelope) {
    throw new Error('verifyEnvelope is not available in the supplied Quip WASM module');
  }

  return wasm.verifyEnvelope(payloadHex, envelopeHex, accountIdHex);
}
