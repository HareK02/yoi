#!/usr/bin/env node
// Verify actual Cargo resolution, not just version strings in workspace manifests.
import assert from 'node:assert/strict';
import { execFileSync } from 'node:child_process';
import { fileURLToPath } from 'node:url';

const cwd = fileURLToPath(new URL('../', import.meta.url));
const host = execFileSync('rustc', ['-vV'], { cwd, encoding: 'utf8' })
  .match(/^host: (.+)$/m)?.[1];
assert.ok(host, 'rustc must report the validation host');
const metadata = JSON.parse(execFileSync('cargo', [
  'metadata', '--locked', '--offline', '--format-version=1', '--filter-platform', host,
], { cwd, encoding: 'utf8', maxBuffer: 16 * 1024 * 1024 }));
const expected = ['wip-client', 'wip-http', 'wip-protocol', 'wip-text-view'];
const source = 'registry+https://github.com/rust-lang/crates.io-index';
const sdk = metadata.packages.filter(pkg => pkg.name.startsWith('wip-'));
for (const pkg of sdk) {
  assert.equal(pkg.version, '0.2.0', `${pkg.name}: no old WIP version is permitted`);
  assert.equal(pkg.source, source, `${pkg.name}: must resolve from crates.io, not a path/Git substitute`);
}
for (const name of expected) {
  assert.equal(sdk.filter(pkg => pkg.name === name).length, 1, `${name}: expected one package source`);
}
console.log(`Verified ${expected.join(', ')} at crates.io 0.2.0; one Protocol source (${host})`);
