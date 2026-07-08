---
title: 'Migrate Profiles to Decodal ProfileSourceArchive for Runtime launch'
state: 'queued'
created_at: '2026-07-07T20:51:35Z'
updated_at: '2026-07-08T09:13:34Z'
assignee: null
queued_by: 'workspace-panel'
queued_at: '2026-07-08T09:11:22Z'
---

## 背景

Runtime の Browser/Backend 経由 Worker creation は、Workspace filesystem を profile/config discovery source として読まない境界にする必要がある。現状の Lua Profile 経路は `ProfileRuntimeWorkerFactory` / `resolve_runtime_profile_manifest` が Runtime-local path を `workspace_root` / `profile_base_dir` として扱い、そこから `.yoi/profiles.toml`、Lua Profile file、local require module、`.yoi/override.local.toml` を探索・読み取りしている。

Lua Profile は sandbox されているが、設定 DSL としては過剰であり、`require` / local module / FS discovery の境界が複雑になっている。Profile は Workspace/Profile source graph として Backend が正本を持ち、Runtime はその source graph を filesystem ではなく archive artifact として受け取り、決定的に評価できればよい。

本 Ticket では Lua Profile の通常経路を Decodal Profile DSL に移行し、Backend が selected Profile revision の source graph を tar archive (`ProfileSourceArchive`) として構築する。Runtime は archive を prefetch / verify し、Decodal `SourceLoader` を archive 内 source に限定して Profile を resolve する。

## 実装順序

1. 本 Ticket `00001KWZ5KERY`: Decodal Profile DSL、`ProfileSourceArchive`、Runtime archive prefetch/verify、FSなし profile resolution を実装する。
2. Ticket `00001KX0DSMPT`: Workspace settings API/pages を Decodal Profile source 編集向けに実装する。Profile source model / validation が本 Ticket で固まった後に進める。
3. Runtime-local artifact sync は別 Ticket とする。Plugin package、MCP executable、sandbox image など、Runtime-local 実体が必要なものだけを対象にする。

## 実装目的

- Lua Profile を Browser/Backend Worker launch の通常経路から外し、Decodal Profile DSL を使う。
- Backend は Workspace/Profile source の正本として Profile source graph を読み、tar 化した `ProfileSourceArchive` を作る。
- Runtime は Workspace filesystem / WorkingDirectory root から Profile discovery を行わず、prefetch 済み archive だけを読む。
- Decodal import は Backend への都度 fetch ではなく、archive manifest が許可する source graph 内で解決する。
- WorkingDirectory は Worker execution root/cwd としてのみ使う。
- Memory / Ticket / Objective のような更新頻度が高い Workspace product state は Runtime に同期せず、Backend API / capability / tool 経由でアクセスする。
- Plugin package、MCP executable、sandbox image など Runtime-local 実体が必要な artifact は Profile source archive と分離する。

## 実装内容

### 1. Decodal Profile DSL を導入する

Lua Profile の通常経路を置き換える Decodal profile schema を追加する。

実装すること:

- Decodal dependency を追加する。
- Worker manifest/config に必要な typed schema を Decodal materialization target として定義する。
- builtin role Profiles を Decodal source へ移行する。
- Profile selector / role metadata / model/provider refs / tool policy / memory/ticket/language policy / prompt/resource refs を Decodal から表現できるようにする。
- secret values、host absolute path、Runtime endpoint、socket/session path、runtime-local mount actual path は schema validation で拒否する。
- Lua Profile は compatibility-only path とし、Browser/Backend launch の通常経路では使わない。

### 2. `ProfileSourceArchive` tar format を定義する

Backend が Runtime に渡す profile source graph artifact を tar archive として定義する。

archive の例:

```text
manifest.json
sources/root.dcl
sources/modules/foo.dcl
sources/modules/bar.dcl
```

manifest に含める情報:

- archive kind / version。
- workspace id。
- profile selector。
- profile revision。
- root source key / root path。
- source entries: key, relative archive path, digest, size, content type。
- import map / import policy。
- builtin profile version / source provenance。
- archive digest。

禁止する情報:

- secret values。
- host absolute path。
- Runtime endpoint / token / socket path / session path。
- WorkingDirectory root path。
- raw Browser cwd / raw filesystem scope。
- Ticket / Objective / Memory contents。
- Plugin executable/package bytes。

### 3. Backend に archive builder を実装する

Workspace Backend が profile/role selector から `ProfileSourceArchive` を作る。

実装すること:

- workspace/project/builtin Decodal Profile registry discovery を Backend 側で行う。
- selected source と import closure を読み取る。
- import specifier を Backend 側で解決し、許可された source graph に閉じる。
- artifact size / file count / import depth / unsupported source / symlink escape / path escape を validation する。
- source digest と archive digest を計算する。
- archive manifest と tar bytes を作る。
- Browser launch options は profile/role candidates だけ返し、archive content / digest / Runtime internal identity は返さない。

### 4. Runtime に archive prefetch / verify / cache を実装する

Runtime Worker creation は `ProfileSourceArchiveRef` を受け取り、archive が無ければ Backend から prefetch する。

実装すること:

- `CreateWorkerRequest` または Backend-facing wrapper に `ProfileSourceArchiveRef` を追加する。
- Runtime は archive digest / manifest schema / source digests / total size / source paths を検証する。
- archive path は relative のみ許可し、absolute path / `..` / symlink escape を拒否する。
- same digest の archive は idempotent に cache / reuse できる。
- missing archive は Backend から fetch して repair できる。
- fetch failure / digest mismatch / manifest invalid / source missing / unsupported archive version は typed diagnostic にする。

### 5. Decodal `SourceLoader` を archive 内 source に限定する

Runtime は Decodal evaluator を持つが、SourceLoader は filesystem や Backend 都度 fetch を行わない。

実装すること:

- `ArchiveSourceLoader` を実装する。
- `SourceLoader::load(current_key, specifier)` は archive manifest / import map だけを参照する。
- import が manifest 許可外に出る場合は typed diagnostic にする。
- Decodal evaluation に max imports / max total bytes / max depth / timeout を設定する。
- resolved Worker manifest/config は既存 validation boundary を通す。

### 6. Worker creation を archive-resolved config に接続する

Runtime Worker creation は、通常経路で Runtime-local filesystem から Profile discovery しない。

実装すること:

- Backend は Worker launch 時に selected Profile revision に対応する `ProfileSourceArchiveRef` を Runtime に渡す。
- Runtime は archive を verify し、Decodal で resolved Worker manifest/config を構築する。
- WorkingDirectory は execution root/cwd/scope の計算にのみ使う。
- Runtime-local profile fallback は Browser/Backend launch の通常経路では使わない。
- Worker metadata / audit に archive id / digest / source graph digest summary を残す。

### 7. Runtime-local artifact sync を別境界に分離する

Profile source archive と Runtime-local artifact sync を混ぜない。

Runtime-local sync/check 対象:

- Plugin package / plugin binary。
- MCP server executable / service package。
- sandbox image / toolchain / runtime-local service。
- large prompt/resource asset のうち Runtime local file として必要なもの。

Runtime-local sync/check 対象ではないもの:

- Profile registry / Profile source archive 以外の Workspace product state。
- Memory / Ticket / Objective / Workspace metadata。
- Repository contents。

Profile から plugin enablement / package refs が出る場合、Runtime は package availability を別途 check し、missing / unsupported / digest mismatch を typed diagnostic にする。

### 8. compatibility fallback を通常経路から外す

既存 CLI / tests / builtin debugging のために Lua/runtime-local profile discovery fallback が残る場合は、compatibility-only として隔離する。

実装すること:

- Browser / Workspace Backend launch では fallback を使わない。
- fallback path は test name / diagnostic / code comment で compatibility-only と明示する。
- `profile_base_dir` / legacy `workspace_root` 呼称が残る場合、通常 launch 経路から参照されていないことを test で確認する。

## 受け入れ条件

- Decodal Profile DSL が Worker launch config を表現できる。
- builtin role Profiles が Decodal source として利用できる。
- `ProfileSourceArchive` tar format と manifest schema が実装されている。
- Backend が profile/role selector から `ProfileSourceArchive` を構築できる。
- Backend archive builder が import closure を解決し、archive path / symlink / size / depth / unsupported source を検証する。
- Runtime が `ProfileSourceArchiveRef` から archive を prefetch / verify / cache できる。
- Runtime `ArchiveSourceLoader` が archive manifest 内の source だけから Decodal import を解決する。
- Runtime Worker creation は通常経路で Workspace filesystem / profile base / WorkingDirectory root から Profile discovery を行わない。
- WorkingDirectory は Worker execution root/cwd としてのみ使われる。
- Browser-facing API に archive content、archive digest、Runtime store path、raw path、Runtime endpoint が露出しない。
- Memory / Ticket / Objective は sync 対象ではなく、Backend API / capability / tool 経由でアクセスする方針が code/docs/tests 上で崩れていない。
- Plugin/MCP/sandbox など Runtime-local artifact availability は `ProfileSourceArchive` とは別の check/sync 境界として扱われる。
- Missing profile、ambiguous selector、invalid Decodal source、missing import、import outside graph、artifact too large、archive digest mismatch、unsupported plugin package、missing runtime-local artifact は typed diagnostic になる。
- Focused tests が Backend archive build、Runtime archive verify、ArchiveSourceLoader import resolution、Worker create without Runtime FS discovery、WorkingDirectory launch success、Browser payload redaction、compatibility fallback isolation を確認する。
- `cargo test -p worker-runtime --features ws-server,fs-store` が通る。
- `cargo test -p yoi-workspace-server` が通る。
- `cargo check -p yoi` が通る。
- `cd web/workspace && deno task check && deno task test` が通る。
- `yoi ticket doctor` が通る。
- `nix build .#yoi --no-link` が通る。

## 非目標

- Secret value synchronization。
- Ticket / Objective / Memory / repository contents / Worker catalog の Runtime 同期。
- Plugin package manager / signature policy の完成。
- Multi-tenant auth model の完成。
- Browser-facing Profile editor UI。これは `00001KX0DSMPT` で扱う。
- Runtime が Workspace filesystem を profile/config discovery source として読む経路の延命。
- WorkingDirectory materializer の新しい dirty snapshot / cache / remote clone policy。
