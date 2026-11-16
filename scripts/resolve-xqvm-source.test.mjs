import assert from 'node:assert/strict';
import test from 'node:test';
import { resolveXqvmSource } from './resolve-xqvm-source.mjs';

const commit = '4a31c966fb46a4ae6588d5f39ca60b162059d004';
const repository = 'https://gitlab.com/quip.network/xquad';
const git = { name: 'xqvm', version: '0.5.0', source: `git+${repository}.git?rev=${commit}#${commit}` };
const registry = {
  name: 'xqvm', version: '0.5.0', repository,
  source: 'registry+https://github.com/rust-lang/crates.io-index',
  manifest_path: '/cargo/xqvm-0.5.0/Cargo.toml',
};
const graph = pkg => ({ packages: [{ name: 'unrelated' }, pkg] });

test('draft git pin fetches the locked commit, without inventing a release tag', () => {
  assert.deepEqual(resolveXqvmSource(graph(git)), { version: '0.5.0', repository, commit, ref: commit });
});

test('same-version registry source uses its own release tag and published VCS commit', () => {
  const published = 'a99f7d2e8956b70453dc6d5e4c1b1705a27d9802';
  assert.deepEqual(resolveXqvmSource(graph(registry), path => {
    assert.equal(path, '/cargo/xqvm-0.5.0/.cargo_vcs_info.json');
    return { git: { sha1: published } };
  }), { version: '0.5.0', repository, commit: published, ref: 'refs/tags/v0.5.0' });
});

test('missing or same-version split xqvm sources fail closed', () => {
  assert.equal(git.version, registry.version);
  assert.notEqual(git.source, registry.source);
  assert.throws(() => resolveXqvmSource({ packages: [] }), /exactly one/);
  assert.throws(() => resolveXqvmSource({ packages: [git, registry] }), /exactly one/);
});

test('moving refs, short revisions and mismatched lock commits are rejected', () => {
  for (const query of ['branch=main', `rev=${commit.slice(0, 7)}`, `rev=${'a'.repeat(40)}`]) {
    assert.throws(() => resolveXqvmSource(graph({ ...git, source: `git+${repository}?${query}#${commit}` })), /full resolved commit/);
  }
});

test('foreign repositories and unproven local paths are rejected', () => {
  assert.throws(() => resolveXqvmSource(graph({ ...git, source: git.source.replace('quip.network', 'another-group') })), /unexpected/);
  assert.throws(() => resolveXqvmSource(graph({ ...registry, repository: 'https://example.com/xquad' })), /unexpected/);
  assert.throws(() => resolveXqvmSource(graph({ ...git, source: null })), /unsupported/);
});

test('registry prereleases and absent or malformed VCS records are rejected', () => {
  assert.throws(() => resolveXqvmSource(graph({ ...registry, version: '0.5.0-dev' })), /stable release/);
  assert.throws(() => resolveXqvmSource(graph(registry), () => ({})), /missing or invalid/);
  assert.throws(() => resolveXqvmSource(graph(registry), () => ({ git: { sha1: 'short' } })), /missing or invalid/);
  assert.throws(() => resolveXqvmSource(graph(registry), () => { throw new Error('missing published record'); }), /missing published record/);
});
