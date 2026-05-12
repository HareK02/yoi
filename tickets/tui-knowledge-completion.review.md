# Review: TUI `#` Knowledge 補完の未実装解消

対象実装: `7b8eb3a feat(pod): wire knowledge slugs into # completion` (branch `tui-knowledge-completion`)

## 前提・要件の確認

要件4項目すべてを diff 上に対応コードと根拠付きで確認した。

- 「`.insomnia/knowledge/` 配下の slug が `model_invokation` の真偽に関わらず列挙される」: `memory::list_knowledge_slugs` が `walk_knowledge` を `model_invokation` フィルタなしで通す（`crates/memory/src/resident.rs:47-52`）。テスト `list_slugs_returns_all_regardless_of_model_invokation`（同 `:196-204`）が `true/false/true` の 3 件を全部返すことを確認している。
- 「`#foo` の prefix 入力中は prefix マッチのみが返る」: `PodSharedState::list_knowledge_completions` が `starts_with` で絞り込み（`crates/pod/src/shared_state.rs:102-113`）。テスト `knowledge_completions_filter_by_prefix`（同 `:262-287`）が `alpha` prefix で `alpha`/`alphabet` のみ返り、`zzz` で空、空 prefix で全件、を確認。
- 「memory_layout が None の Pod で空ベクタ、エラーにならない」: `Pod::knowledge_completions` が `memory_layout.as_ref().map(...).unwrap_or_default()` で短絡（`crates/pod/src/pod.rs:1240-1245`）。controller も `Vec<String>` を素通しで `set_knowledge` に渡すだけで panic 経路なし（`crates/pod/src/controller.rs:391-396`）。IPC 側も `OnceLock` 未 set で空を返す（テスト `knowledge_completions_empty_when_unset` `:256-260` で確認）。
- 「確定時挙動は既存 TUI のまま、Pod 側を埋めるだけ」: TUI クレートには変更なし。IPC server の `CompletionKind::Knowledge` 分岐のみが `Vec::new()` から実装に差し替えられている（`crates/pod/src/ipc/server.rs:105-113`）。`CompletionEntry` の `value` に slug、`is_dir: false` を詰める形は Workflow 分岐と完全に同形。

単体テスト 4 項目もすべて存在し、`cargo test -p memory --lib resident::` と `cargo test -p pod --lib shared_state::` でグリーン。

## アーキテクチャ・スコープ

- 列挙 API を `memory` クレート（低レベル workspace 操作の所在）に追加し、Pod 層は Memory layout から「引いて詰めるだけ」というレイヤ分割を保っている。`llm-worker` を巻き込まない、higher-level は上層という方針に合致。
- `WorkflowCandidate` / `set_workflows` / `list_workflow_completions` と `KnowledgeCandidate` / `set_knowledge` / `list_knowledge_completions` がフィールド順・docstring の有無・実装行数まで揃っており、mirror 対象（`shared_state.rs:74-89`、`controller.rs:385-390`、`pod.rs:1236`）のスタイルと一貫している。
- `walk_knowledge` の共通化は `FnMut(String, KnowledgeFrontmatter)` 1 つの最小抽象で、2 呼び出し元の重複（`read_dir` のエラー早抜け、非 `.md` スキップ、frontmatter parse スキップ）を素直にまとめている。やりすぎ（Iterator 化、ジェネリック化）はしておらず、CLAUDE.md の「変更量を最小に」「設計を歪めない」に合致する。逆に共通化を見送って書き写すと 30 行程度の同形コード重複になるので、この程度の抽出は妥当。
- 範囲外は守られている。frontmatter スキーマ、`collect_resident_knowledge` の挙動（`model_invokation: true` のみ返す resident 注入用途）、workflow/file 補完経路、TUI コードへの手入れは一切なし。

## 指摘事項

### Non-blocking / Follow-up

- なし。

### Nits

- `crates/memory/src/resident.rs:26-28` の `collect_resident_knowledge` の docstring が `<workspace>/knowledge/*.md` のままで、実 path `<workspace>/.insomnia/knowledge/*.md`（`list_knowledge_slugs` 側の docstring `:43-46` では正しく書かれている）と齟齬がある。本チケットの範囲外の既存記述だが、隣接行を編集したついでに同期しておくと整う。今回は追わなくてよい。

## 判断

Approve — ticket の前提・方針・要件・テスト 4 項目すべてに対応コードとパスする単体テストがあり、範囲外を踏まず、Workflow 経路と整合した最小実装になっている。
