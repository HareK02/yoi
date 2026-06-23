Protocol crate 由来の Workspace web TypeScript type generation を実装し、Orchestrator worktree の `orchestration` branch に統合した。

主な成果:
- `crates/protocol` の `stream` module / tokio dependency を default `stream` feature に分離し、DTO-only build で tokio を不要にした。
- Optional `typescript` feature と `ts-rs` による TypeScript export を追加。
- Protocol DTOs に `cfg_attr(feature = "typescript", derive(ts_rs::TS))` を追加。
- Deterministic generator を追加:
  - `crates/protocol/src/typescript.rs`
  - `crates/protocol/examples/generate_typescript.rs`
- Drift check を追加:
  - `cargo test -p protocol --features typescript generated_protocol_types_are_current`
- Generated artifact を追加:
  - `web/workspace/src/lib/generated/protocol.ts`
- Workspace web が generated root protocol types を re-export:
  - `PodProtocolMethod`
  - `PodProtocolEvent`
  - `PodProtocolSegment`
- Workspace backend extension-point notes に、browser が Pod Unix socket に直接接続せず、将来の backend proxy が Worker identity と method allow/block boundary を enforce する方針を記録。
- `Cargo.lock` と `package.nix` cargo hash を更新。

統合・検証:
- Merge commit: `9728b533 merge: protocol typescript generation`
- Implementation commit: `a13fb693 protocol: generate workspace TypeScript types`
- Reviewer final verdict: approve
- Validation passed: `cargo fmt --check`, `git diff --check HEAD^1..HEAD`, `cargo test -p protocol`, `cargo test -p protocol --features typescript generated_protocol_types_are_current`, `cargo test -p protocol --features typescript`, `cargo check -p protocol --target wasm32-unknown-unknown --no-default-features`, `deno task check`, `deno task build`, `cargo run -p yoi -- ticket doctor`, and `nix build .#yoi --no-link`。

既知の非ブロッキング事項:
- `ts-rs` は `#[serde(other)]` on `Segment::Unknown` に warning を出すが、generated artifact には `{ "kind": "unknown" }` が含まれ、current validation は pass。
- 一部 `Option<T>` + `skip_serializing_if` fields は TS で optional field ではなく required nullable に出る。将来 UI が該当 field を使う際は注意。