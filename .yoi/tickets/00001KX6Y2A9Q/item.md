---
title: 'Implement WorkerFilesystemAuthority for no-workdir Workers'
state: 'planning'
priority: 'P1'
created_at: '2026-07-10T21:13:49Z'
updated_at: '2026-07-10T21:32:47Z'
assignee: null
---

## 背景

Worker 実装は現在 `workspace_root: PathBuf` と `cwd: PathBuf` を必須で持ち、tools 登録も `worker.cwd()` と `worker.scope()` を基準にしている。workdir あり Worker では `cwd` が実質 working directory として機能しており問題ないが、embedded no-workdir Worker では「filesystem authority が存在しない」状態を型として表現できない。

no-workdir Worker は、内部プロセスの実 cwd や fallback 元が何であっても、Worker authority としては filesystem に一切アクセスできないべきである。仮想 cwd を与えるだけでは、`Glob` / `Grep` の default base、`Bash.current_dir`、scope default、Ticket/memory/workflow path 解決などから再び filesystem 接点が発生し得る。

## 要件

- Worker に filesystem authority の有無を表す明示的な型を導入する。
- workdir あり Worker は local working directory として `root` と `cwd` を持ち、既存の tools default base / Bash cwd / fs view はこの値を使う。
- no-workdir Worker は filesystem authority を `None` として表現し、Worker authority としての cwd fallback を持たない。
- filesystem authority が無い Worker では `Read` / `Write` / `Edit` / `Glob` / `Grep` / `Bash` を登録しない、または確実に利用不能にする。
- `cwd` と `workspace_root` の意味を分離し、filesystem authority は `WorkerFilesystemAuthority` 側に寄せる。
- 既存の `workspace_root: PathBuf` 依存はこのチケットでは全削除しなくてよいが、filesystem authority として使わない境界を明確にする。

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

最小実装では既存 `cwd: PathBuf` を一気に消さず、Worker 構築時に `WorkerFilesystemAuthority` を導入して tools 登録と runtime/embedded spawn 経路から先に参照させる。no-workdir embedded Worker は `None` を渡す。

## 受け入れ条件

- Worker が filesystem authority 有無を型で保持できる。
- workdir あり Worker では、tools の default cwd が `LocalWorkingDirectory.cwd` に一致する。
- no-workdir Worker では core filesystem tools と Bash が model-visible tool surface に現れない、または実行不能であることをテストで確認できる。
- no-workdir Worker 作成時に workspace root / process cwd fallback が filesystem authority として使われない。
- embedded no-workdir Worker の spawn 経路から `WorkerFilesystemAuthority::None` を指定できる。
- 既存の通常 Worker / workdir あり embedded Worker / spawned child Worker の動作が回帰しない。
- `cargo test` と `nix build .#yoi` が通る。
