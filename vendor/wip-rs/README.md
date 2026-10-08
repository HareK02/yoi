# Pinned WIP SDK source

These four packages were produced by `cargo vendor` from upstream
`ssh://git@gitea.hareworks.net/Hare/wip-rs.git` at
**`1cbe03b49e48dd7e0be28b76fbc3c932f8920a36`**:

- `wip-protocol`
- `wip-http`
- `wip-client`
- `wip-text-view`

Upstream still calls this breaking contract **Unreleased** and uses version
`0.1.0`. It is not the old crates.io `0.1.0` contract. The corresponding canonical
reference revision is `6086f3c5ef10aa1464ce9c824750d7f667217bfd`; see upstream
`MIGRATION.md` and Text View crate documentation. The adopted source includes
structured InterfaceReference, scope_ref, Client invalidation/CallContext, and
complete canonical signatures. This is a source snapshot, not a claim that a new
registry release exists.

The root workspace's exact path dependencies and lockfile resolve all WIP types
from this committed snapshot. There are no dependencies on attached Workdir host
paths or on an SSH checkout in CI. This also allows the Nix dependency fetcher to
fetch only normal registry dependencies. The packages are excluded from Yoi's
workspace membership; their upstream public-API tests remain available separately.

Source files, upstream tests, README files, and Cargo-generated normalized
manifests are unchanged. Each package retains the `.cargo-checksum.json` emitted
by Cargo; `scripts/verify-wip-vendor.mjs` verifies every listed file. License texts
are retained in the upstream packages (also copied at this directory's root for
the Text View package's MIT-or-Apache-2.0 license).

## Reproducing or updating

Use a temporary standalone Cargo project with a `[workspace]` table and
`wip-client` / `wip-text-view` dependencies on the exact Git URL/revision above.
Run `CARGO_NET_GIT_FETCH_WITH_CLI=true cargo vendor --manifest-path <temporary-project>/Cargo.toml <temporary-project>/vendor`.
Copy only its four `wip-*` directories here, retaining their checksums. Do not edit
vendored sources or normalize their manifests by hand. Update all four packages,
root `workspace.metadata.wip-upstream`, the lockfile, documentation and Nix
registry-dependency hash together for a future adoption.

Verify with:

```sh
node scripts/verify-wip-vendor.mjs
cargo tree --locked --offline -p worker
cargo test --manifest-path vendor/wip-rs/wip-text-view/Cargo.toml --target-dir target/wip-upstream
```

A future update must reread the canonical reference and migration contracts, not
infer compatibility from the unchanged `0.1.0` version string.
