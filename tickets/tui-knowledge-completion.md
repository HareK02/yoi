# TUI `#` Knowledge 補完の未実装解消

## 背景

TUI 入力欄で `#` を打つと `CompletionKind::Knowledge` で Pod に `ListCompletions` を投げる導線は既にある（`crates/tui/src/input.rs`、`crates/tui/src/app.rs`）。一方 Pod 側 IPC は `crates/pod/src/ipc/server.rs:105` で

```rust
protocol::CompletionKind::Knowledge => Vec::new(),
```

と無条件に空ベクタを返しており、ワークスペースに knowledge エントリがあっても TUI 上では候補が一切出ない。`#` で何も出ないのはフロントマター（`model_invokation`）の問題ではなく、補完経路そのものが未配線。

Workflow 側は同じ構造で既に動いている：

- `Pod::workflow_completions()` → `Vec<String>`（`crates/pod/src/pod.rs:1236`）
- `PodController::start` で `PodSharedState::set_workflows()` に投入（`crates/pod/src/controller.rs:385`）
- IPC が `shared_state.list_workflow_completions(prefix)` を引いて返す

Knowledge も同じ形に揃えれば足りる。

## 前提

- knowledge の物理レイアウトは `<workspace>/.insomnia/knowledge/<slug>.md`（`crates/memory/src/workspace.rs`）。
- memory クレートには `collect_resident_knowledge` があるが、これは `model_invokation: true` のみを返す resident-injection 用 API で、補完用途には不適。
- `#` 補完では「ユーザーが参照可能な knowledge slug すべて」を返したい（`model_invokation` は resident 注入対象のフラグであって参照可否ではない）。
- workflow の `list_user_invocable` に相当する「列挙 API」が memory クレートに無いので追加が要る。

## 方針

- memory クレートに「knowledge slug 一覧を返す」公開 API を追加する。`collect_resident_knowledge` とは別関数とし、`model_invokation` でフィルタしない。frontmatter が壊れているファイルは silently skip する（`collect_resident_knowledge` と同じく Linter が write 時に shape を保証する前提）。
- `PodSharedState` に knowledge 候補スロット（`workflows` と同形の `OnceLock<Vec<...>>`）を追加し、`PodController::start` で memory layout から列挙して投入する。memory layout 未設定の Pod（`Pod::memory_layout` が `None`）では空のまま残す。
- IPC server の `CompletionKind::Knowledge` 分岐を、`shared_state.list_knowledge_completions(&prefix)` を引いて `CompletionEntry { value: slug, is_dir: false }` に詰める実装に置き換える。
- 補完結果の並びは slug 昇順、prefix マッチは `starts_with`（workflow と揃える）。

## 要件

- TUI で `#` を入力した状態で、ワークスペース `.insomnia/knowledge/` 配下に存在する slug（`model_invokation` の真偽に関わらず）が候補として列挙される。
- `#foo` のように prefix 入力中の場合、prefix にマッチする slug のみが返る。
- memory layout が無効な Pod（`memory_layout: None`）では空ベクタが返り、エラーにはならない。
- 確定時の挙動（`replace_with_knowledge_ref` → `Segment::KnowledgeRef` 化、`#slug` チップ表示）は既存の TUI 側の実装をそのまま使う。Pod 側補完を埋めるだけで TUI 改修は不要。
- 単体テストで以下をカバーする
  - 全件列挙（複数 slug、`model_invokation: true/false` 混在で全部返る）
  - prefix フィルタ
  - knowledge ディレクトリ不在時の空ベクタ
  - 壊れた frontmatter / `.md` 以外のファイルがスキップされる

## 範囲外

- knowledge のフロントマター仕様変更や、補完候補に description / model_invokation を載せて TUI で表示する強化（`CompletionEntry.value` 以外の表示は別チケットで検討）
- workflow / file の補完経路への手入れ
- resident 注入経路（`collect_resident_knowledge` の挙動）の変更

## 参照

- 未実装箇所: `crates/pod/src/ipc/server.rs:105`
- mirror 対象: `crates/pod/src/pod.rs:1236`（`workflow_completions`）、`crates/pod/src/shared_state.rs:74-89`、`crates/pod/src/controller.rs:385-390`
- TUI 側既存導線: `crates/tui/src/input.rs:260`、`crates/tui/src/app.rs:281,315`
- 列挙対象: `crates/memory/src/workspace.rs`（`knowledge_dir`）、`crates/memory/src/resident.rs`（参考）

## Review
- 状態: Approve
- レビュー詳細: [./tui-knowledge-completion.review.md](./tui-knowledge-completion.review.md)
- 日付: 2026-05-12
