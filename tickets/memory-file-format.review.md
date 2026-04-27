# Review: メモリ機構: ファイル形式 + Linter 土台

## 前提・要件の確認

### 完了条件マッピング

| # | 条件 | 結果 | 根拠 |
| - | ---- | ---- | ---- |
| 1 | `crates/memory/` 新設、workspace 登録 | OK | `Cargo.toml:13`, `crates/memory/Cargo.toml`, `crates/memory/src/lib.rs:9-15` |
| 2 | memory tool 3 種（read / write / edit）が登録できる | OK | `crates/pod/src/controller.rs:243-252` |
| 3 | 違反 content で複数違反集約の `InvalidArgument`、fs 不変 | OK | `crates/memory/src/tool/write.rs:185-202`, `:237-249`、`linter::format_report` で全 error を 1 メッセージに集約 |
| 4 | 正常 content は通常書き込み | OK | `tool/write.rs:147-163` |
| 5 | `memory/workflow/` 書き込み拒否 | OK | `linter/mod.rs:101-105`、`tool/write.rs:165-181`, `tool/edit.rs:276-298` |
| 6 | 同 slug 新規作成 error | OK | `linter/mod.rs:120-127`, `tool/write.rs:204-222` |
| 7 | `replaced_by` / `requires` の参照切れと循環が error | 部分 | `replaced_by` 不在: OK / `replaced_by` 循環: アルゴリズム実装済みだが実循環ケースの end-to-end テスト無し / **`requires` 整合: 未実装**（`schema/workflow.rs` で型パースのみ） |
| 8 | Pod 有効化で generic tool Scope deny | OK | `pod.rs:1516-1527` (`build_scope_with_memory`)、`scope::deny_write_rules` |
| 9 | 既存ビルド・テスト壊さない | OK | `cargo build --workspace` / `cargo test --workspace` 全 pass、memory 単体 44 テスト pass |

### Linter ルール（要件節）と実装の照合

静的 error:
- frontmatter 必須欠落・型違反: OK (`linter/frontmatter.rs::map_serde_error`、ただし `parse_invalid_status` は `\`open\`` リテラル含有でラフに status 同定する点はやや fragile — 別フィールドが今後 enum を持つ場合は誤分類リスク)
- workflow 書き込み禁止: OK
- 同 slug 新規禁止: OK
- `replaced_by` 実在: OK
- `replaced_by` 循環: 実装あり / テスト不足
- Decisions `status` enum: OK
- slug 文法: OK (`slug::is_valid_slug` + 全網羅 deserialize 経路)
- `model_invokation: true` の description 1024 上限: OK
- 種別ごとの char 硬上限 (summary 20000 / 他 8000): OK

膨張抑制 Warn:
- 大 record × 単一 sources warn: OK
- sources 配列長 warn: OK
- **類似 slug 乱立 (Levenshtein 距離 2 / 3 件以上) warn: 未実装**（`LintWarning::SimilarSlugs` バリアントは定義済みだが、emit 経路無し。`existing.slugs(kind)` は揃っており実装は容易）

## アーキテクチャ・スコープ

### 良い点
- memory 関連は `crates/memory/` に明確に閉じ込められている。`tools/` 側に侵入なし、`pod` 側は `MemoryConfig` 取得 + `build_scope_with_memory` + tool 登録 3 行のみで責務が薄い
- crate 名は `memory` で `insomnia-` プレフィックス無し（命名ルール準拠）
- 依存追加は `cargo add` 由来とみられる版指定で workspace に整合（`crates/memory/Cargo.toml`）
- `schema/`, `linter/`, `tool/` を feature module で分割しており、巨大 `lib.rs` を回避
- LLM への違反伝達は `ToolError::InvalidArgument` のみで完結。Interceptor 拡張・retry message 注入・違反カウンタを持たないという設計方針通り
- 公開 API は `lib.rs` 12 行で抑制的、leaky でない
- `_staging/` は意図通り linter 透過 (`workspace::classify` で `Ok(None)`)

### 懸念
- **plan 残存矛盾**: `docs/plan/memory.md:196` に「Phase 2 と同じ CRUD tool + Linter Hook を使う」が残っている。「Linter Hook」は本チケットで否定された旧設計の語彙。GC 節の更新漏れ（4 箇所更新と説明されたが GC 節は対象外だった可能性）
- **deny target = `<workspace_root>/memory`**: `MemoryConfig::workspace_root` が `None` の時 pwd を採用するが、pwd ≠ workspace root の Pod を spawn したケースで deny 対象パスがズレ、generic write が実際の `memory/` を保護しない可能性。ticket 要件は文字通りには満たしているが、「workspace_root 既定値が pwd」前提が manifest 側で文書化されていない。`PodManifest` 化のドクコメントに「workspace_root 未指定 ⇒ Pod 構築時 pwd」と書く程度は欲しい

## 指摘事項

### Blocking
なし。完了条件 7 は部分的だが、`requires` の検証は memory tool 経由で書き込む経路が（ticket の禁止により）存在しないため、実害が無い。CLI / pre-commit が別チケットに切り出されていることと整合する

### Non-blocking / Follow-up
- **類似 slug 乱立 warn 未実装** — `crates/memory/src/error.rs:101` `SimilarSlugs` バリアントは定義済みだが emit されない。ticket 要件節「膨張抑制 Warn」に明記されているので、本ライフサイクル内に追加するか、後続 ticket に明示移管したい
- **`requires` 参照整合チェック未実装** — `lint_workflow_frontmatter` (`linter/mod.rs:235`) は型パースだけで、`requires: Vec<Slug>` の各 slug が実在 Workflow か検査しない。memory tool では到達しないが、ticket 完了条件に文言が残っている。CLI ticket に明示的に移すか、本チケットで `lint_workflow_frontmatter` の引数に `&ExistingRecords` を渡す形で実装するのが筋
- **`replaced_by` 循環の end-to-end テスト追加** — `references.rs::tests::empty_chain_terminates` は smoke test に近い。`A.replaced_by=B`, `B.replaced_by=A` を `existing` に置いた状態で 2 ノード閉路を実検出するテストを足すと完了条件 7 のカバレッジが揃う
- **`docs/plan/memory.md:196`** — 「Linter Hook」を「Linter（memory tool 内 pre-write 検証）」相当に書き換え。GC 節は本チケットの直接対象外でも、用語整合は取りたい
- **`MemoryConfig.workspace_root` 既定値の文書化** — `crates/manifest/src/lib.rs:46-53` のドクコメントに「`None` ⇒ Pod の pwd 採用」「pwd ≠ workspace root の Pod では明示指定が必要」を追記
- **`lint_workflow_frontmatter` を `lib.rs` 公開** — CLI 経路の入口になる関数なので `memory::Linter` と並んで `pub use linter::lint_workflow_frontmatter;` しておくと後続 CLI ticket が触れやすい

### Nits
- `linter/frontmatter.rs::parse_invalid_status` は `\`open\`` 文字列マッチで status enum を識別するため、将来別の enum (`status`-like) が増えた時に誤分類するリスク。コメント注記はあるが、`field: "status"` ハードコード分岐に何らかのテストを足したい
- `tool/edit.rs` の `format_report` / `warning_tail` は `tool/write.rs` の同名関数とロジックがほぼ重複。`tool/mod.rs` に小さなヘルパとして括り出すと DRY に
- `SourceRef.range: [u64; 2]` は frontmatter での扱いが配列だが、構造体名コメントが「inclusive range」とのみ。ドクコメントに「`[start_entry_index, end_entry_index]` 両端含む」を簡潔に書くと session-store との対応が明示される
- `Slug::PartialOrd / Ord` は派生済みだが、`HashMap<Slug, _>` で十分なので `BTreeMap` 用途が現状無い。問題ないが意図メモがあると親切

## 判断

**Approve with follow-up** — 完了条件はほぼ全て充足。アーキテクチャ的に memory 専用クレートへ綺麗に閉じ込められ、`tools` / `pod` への漏出は最小（5 行程度）。LLM 違反伝達も `ToolError::InvalidArgument` 一本で完結し、ticket 設計方針に忠実。残課題（類似 slug warn / `requires` 検証 / 循環 e2e テスト / plan 残存表現 / workspace_root ドキュメント）はいずれも非 blocking で、別 PR / 追跡 ticket への計上で十分。ビルド・テストともに workspace 全 pass。
