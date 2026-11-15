import { readFileSync } from 'node:fs';
import { dirname, join } from 'node:path';
import { pathToFileURL } from 'node:url';

const commitPattern = /^[0-9a-f]{40}$/;
const repository = 'https://gitlab.com/quip.network/xquad';
const normalizeRepository = value => value.replace(/\.git$/, '').replace(/\/$/, '');

// Cargo's locked package graph supplies the actual source, independent of TOML
// field order. For a registry release, Cargo downloads its published VCS record.
export function resolveXqvmSource(metadata, readJson = path => JSON.parse(readFileSync(path, 'utf8'))) {
  const packages = metadata.packages.filter(pkg => pkg.name === 'xqvm');
  if (packages.length !== 1) throw new Error('expected exactly one resolved xqvm package');
  const pkg = packages[0];
  if (pkg.source?.startsWith('git+')) {
    const url = new URL(pkg.source.slice(4));
    const commit = url.hash.slice(1);
    if (!commitPattern.test(commit) || url.searchParams.get('rev') !== commit) {
      throw new Error('xqvm git dependency must pin its full resolved commit');
    }
    url.search = '';
    url.hash = '';
    if (normalizeRepository(url.href) !== repository) throw new Error('unexpected xqvm repository');
    return { version: pkg.version, repository, commit, ref: commit };
  }
  if (pkg.source !== 'registry+https://github.com/rust-lang/crates.io-index') {
    throw new Error('unsupported xqvm source; local paths cannot establish published provenance');
  }
  if (!/^\d+\.\d+\.\d+$/.test(pkg.version)) throw new Error('registry xqvm must be a stable release');
  if (normalizeRepository(pkg.repository ?? '') !== repository) throw new Error('unexpected xqvm repository');
  const vcs = readJson(join(dirname(pkg.manifest_path), '.cargo_vcs_info.json'));
  const commit = vcs.git?.sha1;
  if (!commitPattern.test(commit ?? '')) throw new Error('published xqvm VCS commit is missing or invalid');
  return { version: pkg.version, repository, commit, ref: `refs/tags/v${pkg.version}` };
}

if (process.argv[1] && import.meta.url === pathToFileURL(process.argv[1]).href) {
  try {
    const result = resolveXqvmSource(JSON.parse(readFileSync(process.argv[2], 'utf8')));
    console.log([result.version, result.repository, result.commit, result.ref].join('\t'));
  } catch (error) {
    console.error(`ERROR: could not resolve pinned xqvm source: ${error.message}`);
    process.exitCode = 1;
  }
}
