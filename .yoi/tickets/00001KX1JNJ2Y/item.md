---
title: 'Add ProfileSourceTree virtual filesystem for Decodal imports'
state: 'planning'
created_at: '2026-07-08T19:18:25Z'
updated_at: '2026-07-08T19:19:23Z'
assignee: null
---

## 背景

Decodal Profile は import を持つため、Profile source を単一文字列や RDB-like な正規化モデルだけで扱うと破綻する。Backend は Workspace/Profile source の正本を持つが、Browser / Runtime に host absolute path や raw filesystem authority を出してはいけない。

そのため、Backend-owned な仮想 filesystem abstraction として `ProfileSourceTree` を導入する。ユーザーや Browser editor は file tree / relative path として Decodal source を扱い、Backend はその tree revision から import closure を解決して `ProfileSourceArchive` を生成する。Runtime は `ProfileSourceArchive` の manifest / virtual path だけを見て Decodal import を解決し、Workspace filesystem は読まない。

既存 Ticket `00001KWZ5KERY` は Decodal ProfileSourceArchive migration を扱った。これは closed 済みなので、本 Ticket はその follow-up として virtual FS / source tree layer を追加する。

## 実装順序

- depends_on: `00001KWZ5KERY` — Decodal ProfileSourceArchive / archive verify / Runtime FSなし resolution の基礎。
- 本 Ticket: `ProfileSourceTree` virtual filesystem と import resolver / archive builder を実装する。
- 後続: Workspace/Profile settings editor は、この `ProfileSourceTree` API 上で file tree / plain text or CodeMirror editor を実装する。

## 実装目的

- Decodal import を Backend authority で決定的に解決できる。
- Browser editor は host path ではなく virtual relative path の file tree を編集する。
- Runtime は Workspace filesystem を読まず、archive manifest 内 virtual path だけで import を解決する。
- `ProfileSourceArchive` は `ProfileSourceTree` revision + selected root + import closure の snapshot として生成される。
- 将来の CodeMirror editor / CM integration が source file tree に基づいて実装できる。

## 実装内容

### 1. `ProfileSourceTree` model を追加する

Backend-owned Profile source の仮想 filesystem model を定義する。

含めるもの:

- `source_tree_id`。
- `revision` / generation。
- virtual root metadata。
- files: virtual relative path, content digest, size, content type, editable flag。
- source provenance: builtin / workspace / project / generated。
- diagnostics。

禁止するもの:

- host absolute path。
- symlink target path。
- Runtime endpoint / socket / session path。
- secret values。
- WorkingDirectory root path。

### 2. Virtual path resolver を実装する

Decodal import specifier を `current virtual path + specifier` から tree 内 virtual path へ解決する。

実装すること:

- relative import: `./foo.dcdl`, `../shared/base.dcdl`。
- explicit namespace import: builtin / workspace / project など、採用する namespace を typed enum として扱う。
- path normalization。
- root escape rejection。
- absolute path / URL / unsupported scheme rejection。
- extension/content type validation。
- import depth / file count / total bytes limit。

### 3. `ProfileSourceArchive` builder を `ProfileSourceTree` に接続する

Archive builder は selected root virtual path から import closure を解決し、closure だけを tar に入れる。

実装すること:

- selected profile selector -> root virtual path の解決。
- import closure traversal。
- manifest に virtual path / source key / digest / size / content type / import map を記録。
- archive digest は manifest + source contents の canonical identity にする。
- missing import / ambiguous namespace / duplicate source key / oversized closure を typed diagnostic にする。

### 4. Runtime `ArchiveSourceLoader` の virtual path validation を強化する

Runtime は archive manifest の virtual path だけを信頼境界として扱う。

実装すること:

- archive内 source path は manifest に存在するものだけ許可。
- Decodal `SourceLoader::load(current_key, specifier)` は archive import map / virtual path resolver を使う。
- archive tar path と virtual path の対応を検証する。
- absolute path / `..` / manifest外 import / unsupported namespace を拒否する。

### 5. Backend/Profile settings API を file-tree model に寄せる

Profile settings API は source list だけではなく、将来 editor が使う file tree operation を前提にする。

v0 で追加する operation:

- list files。
- read file。
- write file。
- create file。
- delete file。
- rename/move file は v0 で必要なら追加、不要なら非目標。

Browser-facing API は virtual path と safe display path だけを返し、host path を返さない。

## 受け入れ条件

- `ProfileSourceTree` typed model が実装されている。
- Backend が Decodal source files を virtual relative path の tree として list/read/write できる。
- Backend virtual path resolver が relative import と採用 namespace import を解決できる。
- absolute path、URL、root escape、symlink escape、unsupported scheme、oversized closure は typed diagnostic になる。
- `ProfileSourceArchive` builder が `ProfileSourceTree` revision から selected root の import closure tar を生成する。
- Archive manifest に virtual path / digest / import map が含まれる。
- Runtime `ArchiveSourceLoader` は archive manifest 内 virtual path だけで import を解決し、Workspace filesystem を読まない。
- Browser-facing API に host absolute path、Runtime endpoint、secret value、WorkingDirectory root が露出しない。
- Focused tests が virtual path normalization、import closure build、archive manifest validation、Runtime archive import resolution、Browser payload redaction を確認する。
- `cargo test -p yoi-workspace-server` が通る。
- `cargo test -p worker-runtime --features ws-server,fs-store` が通る。
- `cargo check -p yoi` が通る。
- `cd web/workspace && deno task check && deno task test` が通る。
- `yoi ticket doctor` が通る。
- `nix build .#yoi --no-link` が通る。

## 非目標

- CodeMirror editor UI の完成。
- Decodal formatter / formatter integration。
- Profile source を RDB-like form に正規化する編集 UI。
- Plugin package / MCP executable / sandbox artifact sync。
- Memory / Ticket / Objective / Workflow の API migration。
