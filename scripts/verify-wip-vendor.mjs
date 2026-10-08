#!/usr/bin/env node
// Verify the unmodified Cargo-vendored WIP source snapshot, including its tests.
import { readFileSync } from 'node:fs';
import { createHash } from 'node:crypto';
import { fileURLToPath } from 'node:url';
import { dirname, resolve } from 'node:path';
const root = resolve(dirname(fileURLToPath(import.meta.url)), '..');
const packages = ['wip-client', 'wip-http', 'wip-protocol', 'wip-text-view'];
let files = 0;
for (const name of packages) {
  const directory = resolve(root, 'vendor/wip-rs', name);
  const checksum = JSON.parse(readFileSync(resolve(directory, '.cargo-checksum.json'), 'utf8'));
  for (const [file, expected] of Object.entries(checksum.files)) {
    const actual = createHash('sha256').update(readFileSync(resolve(directory, file))).digest('hex');
    if (actual !== expected) throw new Error(`Changed upstream source: ${name}/${file}`);
    files++;
  }
}
console.log(`Verified ${files} files in ${packages.length} packages from wip-rs 1cbe03b49e48dd7e0be28b76fbc3c932f8920a36`);
