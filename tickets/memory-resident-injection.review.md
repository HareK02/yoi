# Review: メモリ機構 `model_invokation: ON` の常駐注入

## 前提・要件の確認

- 「Pod 起動時に `knowledge/*` を走査し、`model_invokation: ON` の record の description を system prompt に連結」
  - `crates/memory/src/resident.rs:25` の `collect_resident_knowledge` が `<workspace>/knowledge/*.md` を走査し、`KnowledgeFrontmatter` を deserialize して `model_invokation: true` のみ採用、slug 順にソートして返す。`crates/pod/src/pod.rs:567-588` で system prompt 生成時に呼び出され、`crates/pod/src/prompt/system.rs:223-231` で `## Resident knowledge` セクションが trailing 部に追記される。要件充足。
- 「`model_invokation: false` のものは含まれない」
  - `resident.rs:58` で `if fm.model_invokation` 判定。`picks_only_model_invokation_true` テストで担保済み。
- 「Phase 2 Pod では注入しない（consolidation は検索ツール経由）」
  - `Pod::set_resident_knowledge_injection(false)` の lever が用意され、`ensure_system_prompt_materialized` 内で `inject_resident_knowledge` フラグと `manifest.memory.is_some()` の両方を条件に注入。Phase 2 Pod の実装はまだ存在しないため、現時点では「lever は用意されたが呼び出し側がない」状態（後述 Follow-up 参照）。
- 「既存の system prompt 構成（AGENTS.md / scope summary / skills 等）と共存」
  - `append_trailing_section` で Working boundaries → AGENTS.md → Resident knowledge の順で追記。`trailing_section_renders_resident_knowledge_entries` テストで順序検証あり。共存 OK。
- 「予算はシステムプロンプト全体予算に含める。`memory/summary.md` の 5k 枠とは別管理にしない」
  - 別バジェット管理は導入されていない。要件通り。
- 「初期は件数キャップ / 優先順位ルール不要」
  - 単純に slug 順で全件出力。要件通り。

## アーキテクチャ・スコープ

- レイヤ境界: 走査ロジックは `memory` クレートに置かれ、`pod` 側は `memory::collect_resident_knowledge` を呼び出すだけ。レイヤ責務に整合。
- catalog 拡張: `PodPrompt::ResidentKnowledgeSection` の追加と `internal.toml` の対応エントリは既存パターン（`AgentsMdSection` 等）に揃っている。`ALL` / `KEYS` の同期と build-time 検査がそのまま機能する。
- prompt rendering: 文字列フォーマットは `format_resident_knowledge_entries` として system.rs にローカル化。テンプレートは `entries` を pre-formatted で受け取るので、後で「list 以外の表現にしたい」になっても catalog 側の差し替えで済む（チケットの「フォーマットは初期 simple リスト、後で再検討」と整合）。
- スコープ膨張は感じられない。新規追加は約 200 行で、要件達成のために必要な最小構成に近い。

## 指摘事項

### Blocking

なし。

### Non-blocking / Follow-up

- `manifest.memory` の `workspace_root` 解決ロジック（`mem.workspace_root.clone().unwrap_or_else(|| pwd.clone())` + `WorkspaceLayout::new`）が今回の追加で 3 箇所に増えた:
  - `crates/pod/src/controller.rs:244-248`
  - `crates/pod/src/pod.rs:567-577`（今回追加）
  - `crates/pod/src/pod.rs:1567-1577`（`build_scope_with_memory`）
  これ自体は本チケットで生まれた重複ではなく既存パターンの踏襲だが、3 箇所目に達した時点で `MemoryConfig::workspace_layout(pwd: &Path) -> WorkspaceLayout` のような小さなヘルパに寄せておくと健全。本チケット範囲外で OK。
- 統合テスト: 単体では `collect_resident_knowledge` と `append_trailing_section` の挙動が個別に担保されているが、Pod の `ensure_system_prompt_materialized` を通る経路（`knowledge/*.md` を置いた状態で system prompt が組み上がるところまで）の end-to-end 確認はない。回帰防止としては unit 2 種で十分カバー範囲に入っているとも読めるが、`inject_resident_knowledge=false` / `manifest.memory=None` の枝を実際の Pod 経路で踏むテストがあると配線ミスを早期に検知できる。
- Phase 2 Pod 自体が未実装で、`set_resident_knowledge_injection(false)` を呼ぶ箇所がない。`tickets/memory-phase2-consolidation.md` 側で「Phase 2 spawn 時にこの setter を呼ぶ」旨を明記しておかないと、将来「lever はあるが誰も呼ばずに常駐注入されてしまう」事故になりうる。Phase 2 チケット側に注記推奨。
- `internal.toml` の `resident_knowledge_section` 文言 ("Use the KnowledgeSearch / MemoryRead tools to fetch the full body when relevant.") はモデル向けの英語固定。多言語 prompt pack を作る運用になった時点で overlay で差し替える前提なので現状で問題ないが、ツール名の改名が起きたら追従が必要（`KnowledgeSearch` / `MemoryRead` が tool catalog 側の正式名と一致しているか確認推奨）。
- `format_resident_knowledge_entries` の改行畳み込み: linter は `description` の長さ上限（1024 chars）は強制するが「単一行」は強制していないので、defensive な `\n` / `\r` → space 変換は妥当な防御。挙動は `trailing_section_renders_resident_knowledge_entries` でカバー済み。

### Nits

- `resident.rs` のテストヘルパ `write_knowledge` は description を `"{description}"` で raw quote しているため、「description に改行が混ざるケース」は単体テストでは触れていない。改行畳み込みは `prompt::system` 側のテストで担保されているので二重には不要だが、collector 側で意図的に `\n` を含む description を 1 件入れて round-trip 確認すると堅牢。
- `pod.rs:567-588` の `resident` / `resident_slice` の二段構えは `Vec` を所有しつつ `Option<&[..]>` を欲しい、という要件のための定石だが、コメントを 1 行足しておくと後続読者に親切（owned `Vec` がスコープを跨ぐ理由）。すでに `// Owned `Vec` lives for the duration of `render` below` の注釈はある。十分。

## 判断

**Approve** — チケットの要件は実装側で漏れなく満たされており、レイヤ責務 / 既存パターンとも整合。Phase 2 側の lever 呼び出しは Phase 2 チケットに引き継ぐ形で問題なく、本チケットの完了条件は満たしている。
