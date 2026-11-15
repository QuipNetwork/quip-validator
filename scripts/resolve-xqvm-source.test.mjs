import assert from 'node:assert/strict';
import test from 'node:test';
import { resolveXqvmSource } from './resolve-xqvm-source.mjs';

const commit = '279cf29bfcc64d09ee08c0d30b098691c24a8c7f';
const repository = 'https://gitlab.com/quip.network/xquad';
const git = { name: 'xqvm', version: '0.5.3-dev', source: `git+${repository}.git?rev=${commit}#${commit}` };
const registry = {
  name: 'xqvm', version: '0.5.3', repository,
  source: 'registry+https://github.com/rust-lang/crates.io-index',
  manifest_path: '/cargo/xqvm-0.5.3/Cargo.toml',
};
const graph = pkg => ({ packages: [{ name: 'unrelated' }, pkg] });

test('draft git pin fetches the locked commit, without inventing a release tag', () => {
  assert.deepEqual(resolveXqvmSource(graph(git)), { version: '0.5.3-dev', repository, commit, ref: commit });
});

test('registry replacement uses its release tag and published VCS commit', () => {
  const published = '1e40da31628026e4cdd47cfae2c0119450044e6c';
  assert.deepEqual(resolveXqvmSource(graph(registry), path => {
    assert.equal(path, '/cargo/xqvm-0.5.3/.cargo_vcs_info.json');
    return { git: { sha1: published } };
  }), { version: '0.5.3', repository, commit: published, ref: 'refs/tags/v0.5.3' });
});

test('missing or split xqvm sources fail closed', () => {
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
  assert.throws(() => resolveXqvmSource(graph({ ...registry, version: '0.5.3-dev' })), /stable release/);
  assert.throws(() => resolveXqvmSource(graph(registry), () => ({})), /missing or invalid/);
  assert.throws(() => resolveXqvmSource(graph(registry), () => ({ git: { sha1: 'short' } })), /missing or invalid/);
  assert.throws(() => resolveXqvmSource(graph(registry), () => { throw new Error('missing published record'); }), /missing published record/);
});
