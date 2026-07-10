---
title: 'Implement WorkerFilesystemAuthority for no-workdir Workers'
state: 'ready'
priority: 'P1'
created_at: '2026-07-10T21:13:49Z'
updated_at: '2026-07-10T21:49:37Z'
assignee: null
---

## 背景

Worker 実装は現在 `workspace_root: PathBuf` と `cwd: PathBuf` を必須で持ち、tools 登録も `worker.cwd()` と `worker.scope()` を基準にしている。workdir あり Worker では `cwd` が実質 working directory として機能しており問題ないが、embedded no-workdir Worker では「filesystem authority が存在しない」状態を型として表現できない。

no-workdir Worker は、内部プロセスの実 cwd や fallback 元が何であっても、Worker authority としては filesystem に一切アクセスできないべきである。仮想 cwd を与えるだけでは、`Glob` / `Grep` の default base、`Bash.current_dir`、scope default、Ticket/memory/workflow path 解決などから再び filesystem 接点が発生し得る。

## 要件

- Worker に filesystem authority の有無を表す明示的な型を導入する。
- Worker の `cwd: PathBuf` property と `worker.cwd()` accessor を削除し、working directory は `WorkerFilesystemAuthority::Local` の中だけで表現する。
- workdir あり Worker は local working directory として `root` と `cwd` を持ち、既存の tools default base / Bash cwd / fs view はこの値を使う。
- no-workdir Worker は filesystem authority を `None` として表現し、Worker authority としての cwd fallback を持たない。
- filesystem authority が無い Worker では `Read` / `Write` / `Edit` / `Glob` / `Grep` / `Bash` を登録しない。
- 既存の `worker.cwd()` 参照箇所をすべて分類し、filesystem authority が必要な箇所は `WorkerFilesystemAuthority::Local` 経由に置き換え、workspace 情報が必要な箇所は `workspace_root`/cwd 依存から切り離す方向で明示的に扱う。
- `workspace_root` はこのチケットでは全削除しなくてよいが、filesystem authority として使わない境界を明確にする。

## 実装メモ

想定する型の方向性:

```rust
enum WorkerFilesystemAuthority {
    None,
    Local(LocalWorkingDirectory),
}

struct LocalWorkingDirectory {
    root: PathBuf,
    cwd: PathBuf,
}
```

Worker 構築時に `WorkerFilesystemAuthority` を必須入力として受け取り、既存の `cwd: PathBuf` / `worker.cwd()` 経路を削除する。no-workdir embedded Worker は `WorkerFilesystemAuthority::None` を渡す。通常 Worker / workdir あり embedded Worker / spawned child Worker は、既存の working directory 解決結果から `LocalWorkingDirectory { root, cwd }` を作って渡す。

## 受け入れ条件

- Worker が filesystem authority 有無を型で保持できる。
- Worker struct から `cwd: PathBuf` field が削除され、`worker.cwd()` accessor も存在しない。
- Worker の constructor / restore / embedded runtime spawn / child spawn 経路は `WorkerFilesystemAuthority` を明示的に受け渡す。
- workdir あり Worker では、tools の default cwd が `LocalWorkingDirectory.cwd` に一致する。
- workdir あり Worker では、authority root が `LocalWorkingDirectory.root` として保持され、cwd と root の意味が分かれている。
- no-workdir Worker では core filesystem tools と Bash が model-visible tool surface に現れないことをテストで確認できる。
- no-workdir Worker では `Read` / `Write` / `Edit` / `Glob` / `Grep` / `Bash` が実行経路上も構築されず、空 scope や実行時エラー頼りの制御になっていない。
- no-workdir Worker 作成時に workspace root / process cwd / runtime cwd fallback が filesystem authority として使われない。
- embedded no-workdir Worker の spawn 経路から `WorkerFilesystemAuthority::None` を指定できる。
- 既存の `worker.cwd()` 利用箇所が残っていないことを grep または同等のテストで確認できる。
- Ticket / memory / workflow / child spawn など、従来 cwd に依存していた箇所は `WorkerFilesystemAuthority::Local` 必須箇所と workspace context 箇所に分類され、no-workdir で local filesystem に触れない。
- 既存の通常 Worker / workdir あり embedded Worker / spawned child Worker の動作が回帰しない。
- `cargo test` と `nix build .#yoi` が通る。
