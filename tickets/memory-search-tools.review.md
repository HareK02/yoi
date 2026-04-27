# Review: メモリ機構: memory / Knowledge 検索ツール

## 前提・要件の確認

### ツール構成
- MemorySearch / KnowledgeSearch を新規 Tool として実装、単一取得は MemoryRead に役割分離 — `crates/memory/src/tool/search.rs`、`tool/mod.rs:21-24` で `read_tool` / `write_tool` / `edit_tool` / `memory_search_tool` / `knowledge_search_tool` を re-export。チケット冒頭の「slug 完全一致は MemoryRead 側」という補正が公開 API・ドキュコメントどちらにも反映されている (`search.rs:31-40`, `read.rs:17-20`)。

### Read/Write/Edit signature 補正（memory-file-format からの繰越）
- `MemoryToolKind` を `tool/mod.rs:28-90` に集約し、Read/Write/Edit 全部で `kind` + `slug` 入力に切り替え済み。`Workflow` を意図的に enum から外しているため、`workflow` を投げた瞬間 deserialize で弾ける（`write.rs:230-243`, `edit.rs:240-253` のテストで担保）。Search 出力 `{slug, kind}` をそのまま Read/Edit に渡せる経路は完了条件どおり成立。
- summary は `slug` 禁止、その他は必須、というルールを `MemoryToolKind::resolve_path` に一箇所で集約しており、3 ツール全部が同じ振る舞いになる。テストも `summary_with_slug_rejected` / `decision_without_slug_rejected` で担保 (`read.rs:172-187`).

### MemorySearch 仕様
- 入力 `query: string` のみ、case-insensitive 部分一致 — `validate_query` で空文字拒否 + lower-case 化 (`search.rs:224-231`)、`scan_text` で行ごとに lower-case 化して比較 (`search.rs:289-300`)。
- 出力 `{slug, kind, excerpt}[]`、summary は slug を `"summary"` 固定で返す — `search.rs:115-122` と `memory_search_finds_summary` テスト。
- 対象/除外ディレクトリ: `summary_path` / `decisions_dir` / `requests_dir` のみ列挙し、`workflow_dir` / `staging_dir` はそもそも触らない構造。`memory_search_excludes_workflow_and_staging` テストで「needle を含めても 0 件」を担保。

### KnowledgeSearch 仕様
- `query` 必須 + `kind` 任意フィルタ。frontmatter が壊れていても本文 grep を継続、frontmatter フィールドは `None` を返す動作 (`search.rs:182-213`)。`knowledge_search_searches_frontmatter_too` で frontmatter 文字列にもヒットすることを担保。
- 出力スキーマ差別化（description / model_invokation を持つ）はチケット改訂版にも記述あり、原チケットの「同型」を読み手にも明示している。

### 共通・ソート・上限
- `list_md_files` がスラグ昇順ソート (`search.rs:256`)、ファイル内は `lines().enumerate()` で出現順 — チケットの「ファイル名昇順 → 行順」と一致。
- `manifest [memory]` から `search_hit_limit` / `search_excerpt_lines` を tune できる経路あり (`crates/manifest/src/lib.rs:54-64`, `config.rs:204-212`, `controller.rs:249-260`)。デフォルト 20 / 3 行で原仕様と一致。
- `memory_search_respects_hit_limit` / `memory_search_excerpt_includes_context_lines` テストで両方が効くことを担保。

### 登録
- `pod/src/controller.rs:256-260` で memory enabled 時にのみ 5 ツールが並列登録される。Phase 2 Pod 側はチケットでも明示的に範囲外と書かれていて整合。

### 完了条件マッピング
- 通常 Pod から 5 ツール呼べる ✅ (`controller.rs`)
- Search → Read 経路成立 ✅（共通 `MemoryToolKind` で型一致）
- frontmatter 含む全文ヒット ✅ (`scan_text` は raw 全体を走査、専用テストあり)
- `kind` filter ✅ (`knowledge_search_kind_filter` テスト)
- 対象外ディレクトリ非ヒット ✅
- tuning 可 ✅

## アーキテクチャ・スコープ

- `crates/memory/src/tool/` にファイル分割で同居、既存 read/write/edit と完全に同じパターン。`MemoryToolKind` を `mod.rs` に置いて 4 ツール全部が共有しているのは適切。
- 依存追加なし（`std::fs` / `serde_yaml` / 既存の `schema::*` のみ）。`cargo add` ルールに抵触しない。
- `ToolDefinition` を `Arc<Fn>` で返す既存ファクトリ規約を踏襲し、`SearchConfig` を closure に move するだけのミニマルな受け渡し。
- `manifest::MemoryConfig` への 2 フィールド追加は `Option<usize>` + cascade で merge する既存パターンに従っている。
- スコープ越境なし（llm-worker / pod / scope の責務境界に変更なし）。「memory crate は self-contained」という既存ポリシー (`lib.rs:1-7`) もそのまま守られている。
- 派生 index を持たず都度スキャンとしている点も、原チケットの「FTS / vector は将来」とブレなし。

## 指摘事項

### Blocking
なし。

### Non-blocking / Follow-up（任意）
- **`search.rs:115-122` の summary 走査が `hit_limit` 上限ガードを通っていない**。現状は `limit - hits.len()` を `scan_file` に渡しているので `limit == 0` でも 0 が伝わって動作上は問題ないが、可読性として decisions / requests と同じ `if hits.len() < limit` ガードを前置きしておく方が一貫し、将来の修正で誤って `usize` underflow を踏み込む隙を消せる。
- **`SearchConfig` 適用ロジックが controller 側に展開コードとして書かれている** (`controller.rs:249-255`)。`MemoryConfig` → `SearchConfig` の変換を `From` impl で memory crate 側に持たせるとテストしやすく、controller を細くできる。manifest を memory に依存させたくない場合は逆向き (`manifest` 側に `to_search_config`) でも可。今のままでも動くので任意。
- **frontmatter の YAML パース失敗時、`kind` filter 指定があるとそのファイルが完全に excluded になる** (`search.rs:189-198`)。仕様上「frontmatter parse 失敗時は本文 grep 継続」と書かれているので、kind filter 指定時のみ skip するのは spec 通り（filter で絞り込みたいユーザーが unknown-kind のファイルを取りたいケースは無い）。ただしドキュコメントに「kind filter ありで frontmatter 壊れているファイルは skip」と一行足しておくと混乱しない。
- **`KnowledgeSearchTool` は frontmatter parse 失敗ファイルも逐次 lower-case 比較するため負荷がやや高い**。本チケットの「都度スキャン」「FTS は将来」前提なので問題視はしないが、`hit_limit` を 0 や巨大値にしたユーザーに対して description などに足切り入れる余地は将来検討。

### Nits
- `MEMORY_SEARCH_DESCRIPTION` / `KNOWLEDGE_SEARCH_DESCRIPTION` が `hit_limit` に言及しているが、tool input にはこのフィールドが無い（manifest 側 tunable）。LLM が `hit_limit` を引数に入れたくならないか軽く心配。「(configurable via manifest)」のような括弧書きで明示してもよい。
- `parse_hits` テストヘルパーが `OwnedMemoryHit` / `OwnedKnowledgeHit` で「Owned」プレフィックスを持つが、テスト内のシリアライズ用 struct を「Owned」と呼ぶ意図が明示されていない。`parsed_*` などでも可。

## 判断

**Approve（完了可）** — 原チケットの要件と完了条件はすべて実装に対応付けが取れており、補正（Read/Write/Edit signature 切り替え・出力スキーマ差別化・slug 完全一致を Read 側へ移譲）はチケット本文にも明確に反映されている。コードベースの既存パターンを踏襲しており、歪みも gold-plating も無し。指摘は全て任意の follow-up。
