# Dependency/license audit report

Date: 2026-06-01

Scope: read-mostly dependency and license audit for the Yoi workspace at `/home/hare/Projects/yoi`, per `artifacts/delegation-intent.md`. I did not modify dependency manifests, source code, lockfiles, docs, or ticket files other than this report artifact. I did not read ignored secret-like file contents.

## Executive summary

No dependency-license incompatibility blocker was identified from the available local metadata. The main release-risk gap is process/packaging: there is no checked-in dependency license policy or generated third-party notices artifact, so a public binary/source release should add one before publication if notices are expected to ship with the release.

The clearest cleanup candidates are non-blocking: normalize `reqwest` TLS features so Yoi does not enable both native OpenSSL TLS and rustls-related TLS paths, align the direct `crossterm` version with `ratatui`'s backend dependency, and periodically review the HTML/YAML parsing stacks for weight/maintenance.

## Methodology and commands used

Evidence came from local manifests, lock/metadata commands, dependency trees, and source usage greps. Commands were read-only except for writing this report.

File reads:

- `artifacts/delegation-intent.md`
- root `Cargo.toml`, `Cargo.lock`, workspace crate `Cargo.toml` files under `crates/*/Cargo.toml`
- `LICENSE`
- `flake.nix`, `package.nix`, `devshell.nix`

Inventory and license commands:

```sh
cd /home/hare/Projects/yoi
cargo metadata --locked --format-version 1
cargo metadata --locked --format-version 1 | jq -r '...direct workspace dependency grouping...'
cargo metadata --locked --format-version 1 | jq -r '...license field grouping and concerning-license filters...'
cargo deny check licenses
cargo deny --locked --offline list -f tsv
cargo deny --locked --offline --all-features list -f tsv
cargo deny --locked --offline --all-features list -f tsv | awk -F '\t' '...license counts...'
cargo deny --locked --offline --all-features list -f tsv | awk -F '\t' '...non-standard/copyleft/notice-relevant license packages...'
cargo tree --locked -e features -i reqwest@0.13.2
cargo tree --locked -i openssl-sys@0.9.112 --all-features
cargo tree --locked -i native-tls@0.2.18 --all-features
cargo tree --locked -i rustls@0.23.37 --all-features
cargo tree --locked --duplicates --all-features
cargo tree --locked -e no-dev --prefix none | sort -u | wc -l
cargo tree --locked -e no-dev --duplicates
```

Source usage checks:

```sh
rg 'use reqwest|reqwest::|ClientBuilder|Client::builder' crates/**/*.rs
rg 'html5ever|markup5ever|RcDom|parse_document' crates/**/*.rs
rg 'serde_yaml|frontmatter|yaml|YAML' crates/**/*.rs
rg 'zstd|encode_all|decode_all' crates/**/*.rs
rg 'mlua::|Lua::|LuaSerdeExt|require\(' crates/**/*.rs
rg 'crossterm::|ratatui::' crates/**/*.rs
```

Fallbacks / tool notes:

- `python3` was unavailable in the audit environment, so JSON processing used `jq` and `awk`.
- `cargo deny check licenses` exited non-zero because no project license policy/config is present; I used `cargo deny ... list -f tsv` as a local metadata fallback rather than treating the policy failure itself as license evidence.
- No web lookup was used. License conclusions are therefore limited to local crates.io metadata / cargo-deny parsing and local manifests.

## Workspace/dependency shape

- Root workspace: 19 crates, workspace package license `MIT`.
- Project license file: `LICENSE` is MIT.
- `cargo metadata --locked --format-version 1` reported 486 package records in the resolved metadata set.
- `cargo tree --locked -e no-dev --prefix none | sort -u | wc -l` reported 404 unique non-dev tree lines.
- Direct external dependencies found across workspace manifests: 45 unique package names including dev/build-only dependencies.

## Direct Rust dependencies and rough purpose notes

### Runtime/build dependencies

| Dependency | Direct users | Rough purpose / usage note |
| --- | --- | --- |
| `arc-swap` | `manifest`, `pod` | Shared mutable configuration/state handles. |
| `async-trait` | `llm-worker`, `memory`, `pod`, `provider`, `tools` | Async trait object ergonomics for tool/provider abstractions. |
| `base64` | `provider` | Codex/OAuth or provider token/body encoding helpers. |
| `chrono` | `lint-common`, `memory`, `pod`, `provider`, `workflow` | Timestamps, serde timestamps, workflow/memory metadata. |
| `clap` | `pod` runtime; `llm-worker` dev | CLI parsing. |
| `crossterm` | `tui` | Terminal input/output/events. Direct version is `0.28`; `ratatui` pulls `0.29` transitively. |
| `eventsource-stream` | `llm-worker` | SSE stream parsing for LLM/provider responses. |
| `fs4` | `pod`, `pod-registry` | Cross-process file locking. |
| `futures` | `llm-worker`; dev in `pod`, `session-store` | Stream/future helpers. |
| `globset`, `ignore`, `grep-matcher`, `grep-regex`, `grep-searcher` | `tools` | Local Glob/Grep tool implementation, gitignore-aware search. |
| `html5ever`, `markup5ever_rcdom` | `tools` | WebFetch HTML parsing and Readability-style extraction (`tools/src/web.rs`). |
| `include_dir` | `pod` | Compile-time embedding of prompt/profile/runtime resources. |
| `libc` | `memory`, `pod`, `pod-registry` | Unix process/permission/runtime details. |
| `minijinja` | `pod` | Prompt/template rendering. |
| `mlua` | `manifest` | Lua profile evaluation with vendored Lua 5.4 and serde integration. |
| `proc-macro2`, `quote`, `syn` | `llm-worker-macros` | Procedural macro implementation. |
| `pulldown-cmark` | `tui` | Markdown rendering/parsing in terminal UI. |
| `ratatui`, `unicode-width` | `tui` | Terminal UI rendering and width calculations. |
| `reqwest` | `llm-worker`, `provider`, `tools` | HTTP client for LLM transport, OAuth refresh, WebSearch/WebFetch. |
| `schemars` | `memory`, `pod`, `tools`; `llm-worker` dev | JSON schema generation for tools/config/test surfaces. |
| `serde`, `serde_json`, `serde_ignored`, `toml` | many crates | Serialization and config/profile parsing. |
| `serde_yaml` | `memory`, `workflow` | YAML frontmatter parsing for memory/workflow/skill documents. |
| `sha2` | `memory`, `secrets`, `tools` | Hashing/audit/secret-integrity and web/cache utilities. |
| `tempfile` | `tools` runtime; many crates dev | Temporary files/directories for command/tool execution and tests. |
| `thiserror` | many crates | Typed error definitions. |
| `tokio`, `tokio-util` | many crates | Async runtime, process/socket/time/file operations, stream utilities. |
| `tracing` | many crates | Structured runtime logging. |
| `uuid` | `client`, `memory`, `pod`, `protocol`, `session-store`, `tui` | Session/run/Pod identifiers; v7 and serde features where needed. |
| `zstd` | `llm-worker` | Codex backend request compression; usage confirmed in `llm_client/transport.rs`. |

### Dev-only direct dependencies

| Dependency | Direct users | Rough purpose / usage note |
| --- | --- | --- |
| `dotenv` | `llm-worker`, `pod` dev | Local dev/test credential loading. Not a runtime dependency. |
| `filetime` | `tools` dev | Filesystem timestamp testing. |
| `serial_test` | `provider` dev | Serializes provider tests that mutate shared state. |
| `tracing-subscriber` | `llm-worker` dev | Test/example logging setup. |
| `trybuild` | `llm-worker` dev | Proc-macro compile-fail/compile-pass tests. |
| `wiremock` | `llm-worker`, `provider` dev | Mock HTTP server for provider/client tests. |

## Transitive/license summary

### Local project license

- Workspace package license: `MIT` in root `Cargo.toml`.
- Repository `LICENSE`: MIT.
- Nix package metadata: `meta.license = lib.licenses.mit`.

### Cargo metadata / cargo-deny findings

`cargo metadata` showed no packages with missing `license` metadata.

`cargo deny --locked --offline --all-features list -f tsv` produced the following license-column counts. Counts include packages that offer multiple license alternatives, so the total exceeds the number of packages.

| License column | Count |
| --- | ---: |
| MIT | 348 |
| Apache-2.0 | 254 |
| Unicode-3.0 | 19 |
| Unlicense | 10 |
| Apache-2.0 WITH LLVM-exception | 8 |
| ISC | 7 |
| BSD-3-Clause | 2 |
| LGPL-2.1-or-later | 2 |
| BSD-2-Clause | 1 |
| BSL-1.0 | 1 |
| CC0-1.0 | 1 |
| CDLA-Permissive-2.0 | 1 |
| MIT-0 | 1 |
| OpenSSL | 1 |
| Zlib | 1 |

### Unknown, missing, copyleft, non-standard, or notice-relevant licenses

No missing license metadata was observed locally.

Items to explicitly account for in a release license policy/notice flow:

- `r-efi@5.3.0` and `r-efi@6.0.0` have expression `MIT OR Apache-2.0 OR LGPL-2.1-or-later`. This is not a blocker if the project selects the permissive MIT/Apache alternative, but a policy tool should encode that choice so the LGPL alternative is not misread as an obligation.
- `aws-lc-sys@0.35.0` is reported with `ISC AND (Apache-2.0 OR ISC) AND OpenSSL`; `aws-lc-rs`, `rustls`, `hyper-rustls`, `rustls-native-certs`, `rustls-webpki`, and `untrusted` also show ISC-family entries. The OpenSSL marker is notice-relevant and should be included in third-party notices if that path remains enabled.
- `webpki-root-certs@1.0.5` is `CDLA-Permissive-2.0`; include in notices/policy.
- ICU4X-related crates (`icu_*`, `zerovec*`, `zerofrom*`, `yoke*`, `tinystr`, `litemap`, `writeable`, `potential_utf`, `unicode-ident`) carry `Unicode-3.0`; include in policy/notices.
- `rustix`, `linux-raw-sys`, `wasi`, `wasip2`, `wasip3`, `wit-bindgen` include `Apache-2.0 WITH LLVM-exception`; standard permissive but notice-relevant.
- `globset`, `ignore`, `grep-*`, `aho-corasick`, `memchr`, `same-file`, `walkdir`, `winapi-util` include `Unlicense OR MIT`; select MIT or otherwise account for Unlicense acceptance in policy.
- `encoding_rs` / `subtle` show BSD-3-Clause, `zerocopy` shows BSD-2-Clause alternative, `ryu` shows BSL-1.0 alternative, `foldhash` shows Zlib, and `dunce` shows CC0/MIT-0/Apache alternatives. These are not blockers but should be represented in generated notices.

A broader `cargo metadata` license-field scan also surfaced `terminfo@0.9.0` with `WTFPL`, but `cargo tree` did not show it in the active default/all-features tree for Yoi; reverse metadata edges point through optional `termwiz`/`ratatui-termwiz`. I would not treat this as a release blocker based on current tree evidence, but a future policy check should confirm inactive optional dependencies are excluded or explicitly allowed.

## Heavy/redundant/replaceable dependency candidates

### 1. `reqwest` TLS feature duplication — high confidence cleanup candidate

Evidence:

- `llm-worker` and `tools` declare `reqwest` with `default-features = false` plus `native-tls`.
- `provider` declares `reqwest = { version = "0.13", features = ["json", "native-tls"] }` without `default-features = false`.
- `cargo tree --locked -e features -i reqwest@0.13.2` showed `provider` enabling `reqwest feature "default"`, which then enables `default-tls` and rustls-related features, while `native-tls` is also enabled.
- Inverse trees showed both `openssl-sys -> native-tls -> hyper-tls -> reqwest` and `rustls -> hyper-rustls -> reqwest` in the graph.

Impact:

- Larger dependency graph and binary/build surface.
- Keeps Nix `openssl`/`pkg-config` system dependency necessary via native TLS.
- Adds license/notice surface from both native TLS/OpenSSL and rustls/aws-lc paths.

Recommendation:

- Open a follow-up to choose one TLS policy for Yoi HTTP clients. If native certificate store behavior is required, encode that intentionally. If rustls is sufficient, remove native OpenSSL TLS and revisit Nix `openssl`/`pkg-config` inputs. If native TLS is preferred, disable `reqwest` defaults consistently so rustls/default TLS paths are not accidentally enabled.

### 2. Duplicate `crossterm` versions — high confidence cleanup candidate

Evidence:

`cargo tree --locked --duplicates --all-features` shows:

```text
crossterm v0.28.1
└── tui v0.1.0

crossterm v0.29.0
└── ratatui-crossterm v0.1.0
    └── ratatui v0.30.0
        └── tui v0.1.0
```

Impact:

- Duplicate terminal backend stack and some duplicated transitive platform crates.
- Likely avoidable by aligning direct `crossterm` with `ratatui`'s backend dependency, if API changes are small.

Recommendation:

- Open a small cleanup ticket to update direct `crossterm` to the version used by `ratatui-crossterm`, run TUI/input tests, and remove the duplicate if compatible.

### 3. HTML extraction stack (`html5ever` + `markup5ever_rcdom`) — medium confidence review candidate

Evidence:

- Direct dependency in `tools`.
- Usage is localized to WebFetch HTML parsing/extraction in `crates/tools/src/web.rs` (`html5ever::parse_document`, `RcDom`).
- Duplicate tree evidence includes older `syn v1`, `phf_*`, `siphasher`, `markup5ever`, and related build-time transitive crates from this stack.

Impact:

- This is a relatively heavy parser stack for one subsystem, but it implements a real product requirement: robust local HTML extraction without sending raw HTML to the model.

Recommendation:

- Do not remove opportunistically. Open a follow-up only if WebFetch binary size/build time becomes a priority; compare with a maintained lighter parser/extractor while preserving safety behavior.

### 4. `serde_yaml` frontmatter parsing — medium confidence maintenance review candidate

Evidence:

- Direct users: `memory`, `workflow`.
- Usage is frontmatter/skill/workflow parsing and linting.

Impact:

- YAML is appropriate for frontmatter, but the Rust YAML ecosystem has maintenance caveats. This is not a license blocker from local metadata.

Recommendation:

- Non-blocking follow-up: decide whether to keep `serde_yaml`, switch to a maintained fork, or constrain frontmatter to a smaller parser-supported subset. This requires design judgment because it affects user-authored workflow/memory files.

### 5. Dev/test HTTP stack (`wiremock`, `serial_test`, `trybuild`) — low confidence cleanup candidate

Evidence:

- `wiremock` appears only in dev dependencies for `llm-worker` and `provider`.
- `serial_test` and `trybuild` are dev-only.

Impact:

- They increase test dependency graph, not runtime release surface.

Recommendation:

- No release action needed. Only revisit if CI time or test dependency policy becomes a problem.

### 6. `mlua` vendored Lua — low confidence replacement candidate / justified heavy dependency

Evidence:

- Direct user: `manifest`.
- Source usage is concentrated in Lua Profile evaluation (`manifest/src/profile.rs`) with controlled `require("yoi.*")` modules.

Impact:

- Vendored interpreter is non-trivial build surface, but it supports a core profile-authoring direction.

Recommendation:

- Keep. Do not create a replacement ticket unless the product direction away from Lua Profiles is explicitly changed.

## Nix/system dependency notes

### `flake.nix`

- Inputs: `nixpkgs` from `github:nixos/nixpkgs?ref=nixos-unstable`, `flake-utils` from `github:numtide/flake-utils`.
- Outputs expose `packages.default`, `packages.yoi`, `apps.default`, `apps.yoi`, and `checks.default = yoi`.
- No extra system libraries are introduced in `flake.nix`; it delegates to `package.nix`.

### `package.nix`

- Build dependencies:
  - `nativeBuildInputs = [ pkg-config ]`
  - `buildInputs = [ openssl ]` plus Darwin frameworks `CoreFoundation`, `Security`, `SystemConfiguration` on macOS.
- `openssl`/`pkg-config` are consistent with the current native TLS path through `reqwest`/`native-tls`/`openssl-sys`.
- `meta.license = lib.licenses.mit` matches workspace/repo license.
- `cargoHash` is pinned.
- `depsExtraArgs` rewrites cargo vendor fetching from crates.io API download URLs to `static.crates.io` due an upstream/nixpkgs fetcher issue; this is packaging infrastructure, not a license concern.
- Source filter excludes `.git`, `target`, `result`, `.yoi`, `.worktree`, `work-items`, and `docs/report` from package source closure. This reduces accidental release of local coordination/generated state.

### `devshell.nix`

- Dev packages: `nixfmt`, `deno`, `git`, `rustc`, `cargo`.
- Dev build inputs: `pkg-config`, `openssl`.
- These are development/build tools, not bundled runtime dependencies. `deno` is present in the dev shell but not in `package.nix` or Cargo runtime dependencies.

## Release blockers vs non-blocking follow-ups

### Release blockers

No dependency license was identified as incompatible with MIT publication from the local metadata.

Potential process blocker before public distribution: third-party notices/license policy are not currently materialized. Apache-2.0, BSD, Unicode, CDLA, OpenSSL-marker, LLVM-exception, and other permissive licenses are acceptable in principle but should be included in a generated notice/policy artifact for release hygiene.

### Non-blocking follow-ups

- Normalize `reqwest` TLS features and decide whether Yoi wants native TLS/OpenSSL or rustls. This may also simplify Nix system dependencies.
- Align direct `crossterm` with `ratatui`'s `crossterm` backend to remove duplicate versions.
- Add CI-enforced license/dependency policy (`cargo-deny` or equivalent) and generated third-party notices.
- Review HTML parser stack only if size/build time is a concern.
- Review `serde_yaml` maintenance posture for frontmatter parsing; this is design-sensitive, not an obvious cleanup.

## Recommended follow-up tickets

1. **Add dependency license policy and third-party notice generation**
   - Acceptance: checked-in `cargo-deny` or equivalent policy; generated/reproducible notices for release artifacts; explicit choices for dual/multi-license crates such as `r-efi`, `Unlicense OR MIT`, and rustls/aws-lc/OpenSSL-marked crates; CI command documented.

2. **Normalize HTTP TLS backend and Nix OpenSSL dependency**
   - Acceptance: all direct `reqwest` users consistently disable or enable defaults according to one documented TLS policy; dependency tree no longer unintentionally contains both `native-tls`/OpenSSL and rustls/default TLS paths unless intentionally justified; `package.nix` `openssl`/`pkg-config` inputs are retained or removed according to evidence.

3. **Deduplicate TUI terminal backend dependencies**
   - Acceptance: direct `crossterm` version is aligned with `ratatui-crossterm` or duplicate is otherwise justified; TUI/input behavior is validated.

4. **Evaluate frontmatter YAML parser maintenance**
   - Acceptance: decide to keep `serde_yaml`, migrate to a maintained fork, or specify a smaller frontmatter subset; include migration/compatibility implications for `.yoi/workflow`, memory, and skill files.

5. **Optional WebFetch parser weight review**
   - Acceptance: compare current `html5ever`/`RcDom` extractor with viable maintained alternatives; preserve bounded, safe, link-aware extraction behavior; only proceed if measurable binary/build-time benefit exists.
