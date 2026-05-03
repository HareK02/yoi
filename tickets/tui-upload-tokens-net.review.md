# Review: TUI ステータスライン `↑` をキャッシュ控除後の純アップロード量にする

## 前提・要件の確認

- **protocol `Event::Usage` に `cache_read_input_tokens: Option<u64>` を追加**: `crates/protocol/src/lib.rs:284-297` で追加済み。`#[serde(default, skip_serializing_if = "Option::is_none")]` 付与でフォワード/バックワード互換を維持。doc コメント (`lib.rs:284-291`) で「占有量・キャッシュ subset・差分が net upload」という意味付けが明文化されており、ticket の要件記述（最低限 cache_read）に正確に合致。
- **pod controller の passthrough**: `crates/pod/src/controller.rs:228-233` で `worker.on_usage` クロージャから `event.cache_read_input_tokens` をそのまま `Event::Usage` に流している。worker `UsageEvent` (`crates/llm-worker/src/llm_client/event.rs:67-79`) は元々全プロバイダで cache_read を normalize 済みなので、passthrough だけで正しい値が乗る。
- **TUI 累積・表示**: `crates/tui/src/app.rs:469-483` で `input_tokens.unwrap_or(0).saturating_sub(cache_read_input_tokens.unwrap_or(0))` を `run_upload_tokens` に加算。`saturating_sub` 採用は cache_read が混在 normalize の都合で input より大きくなる病的ケースに対するガードとして正しい。`Event::RunEnd` (`app.rs:487-499`) でも同フィールドを `Block::TurnStats` に詰めて 0 リセット。`Block::TurnStats` レンダリング (`crates/tui/src/ui.rs:407-422`) と `draw_status` (`ui.rs:778-793`) も `upload_tokens` / `run_upload_tokens` を参照しており、表示文字列は元と同じ `↑{}/↓{}`。
- **`pod_protocol` example**: `crates/pod/examples/pod_protocol.rs:76-87` でパターンに `cache_read_input_tokens` を追加し、`println!` に `cache_read=` を出している。ビルド整合に必要な最小変更。
- **session-store 不変**: ticket 通り `LogEntry::LlmUsage` 周辺は触っていない。
- **ビルド・テスト**: `cargo check -p protocol -p pod -p tui` 通過 (報告済 + こちらでも再確認、新規警告なし)。

完了条件「protocol/pod/TUI が cache_read 情報を受け渡せる経路ができている」「`Block::TurnStats` の `↑` も同じ意味で揃う」はいずれも満たされている。tool ループ実機検証は user 側マニュアルチェックの範囲だが、計算式自体は ticket の指定通り。

## アーキテクチャ・スコープ

- 変更は **protocol → pod → tui** の縦方向 1 経路に閉じており、llm-worker (低レベル基盤) には触れていない。MEMORY.md の「llm-worker は低レベル基盤」原則に整合。
- 新規クレートなし、依存追加なし。`cargo add` 関連メモは無関係。
- protocol 変更は単一フィールド追加のみで、`Option<u64>` + `serde(default, skip_serializing_if)` により旧 Pod ↔ 新 TUI（フィールド欠落 → `None` → `saturating_sub(0)` で従来挙動）と新 Pod ↔ 旧 TUI（未知フィールドは serde 既定で無視）の両向きに非破壊。互換挙動の判断は妥当。
- フィールド名リネーム (`run_input_tokens` → `run_upload_tokens`、`Block::TurnStats.input_tokens` → `upload_tokens`) は ticket の「フィールド意味も同方向に揃える」という指示を字面通り実装と命名の両方で揃えたもの。元の `input_tokens` 名は意味が変わったあと残すと将来の読み手を misleading するため、リネームしておく方がコードベース健全性として正しい判断。命名のみ変更で外部 API (protocol) には波及していない点も適切。
- ticket 範囲外（料金計算・複数ターン累積・cache hit 率・TUI 以外のクライアント変更）への踏み込みなし。確認: `Event::Usage` を消費する他クレートは llm-worker 系内部の `LlmEvent::Usage(UsageEvent)`（別物）と pod_protocol example のみ。daemon/pod-cli/native-gui 等の追加クライアントは crate として未存在で巻き込みなし。
- `cache_creation_input_tokens` は protocol に **乗せない** 判断: ticket 20 行目「cache_creation は full price 側として上載せされたまま」「表示式 `input - cache_read`」と完全に整合。worker `UsageEvent` には `cache_creation_input_tokens` が存在するが、現 protocol 公開層では誰も使わないので過剰公開を避けたのは適切（必要になったら同じパターンで足せる）。

## 指摘事項

### Blocking
なし。

### Non-blocking / Follow-up
- protocol の doc コメント (`crates/protocol/src/lib.rs:284-291`) に「`input_tokens` は cache 込み占有量である」旨が書かれているが、`output_tokens` と従来の `input_tokens` の意味（worker `UsageEvent` 規約と同じ）について、protocol 公開時点での規約として一段明示があると将来の他クライアント実装者が迷いにくい。今回の修正対象ではないので follow-up でよい。
- `Block::TurnStats` の `output_tokens` フィールドは命名変更されていない（ticket 範囲外）。`upload_tokens` だけ「net」意味に変えたので、対称性を意識するなら将来 `output_tokens` も「output_tokens（cache 概念なし）」のままで OK だが、表示が `↑/↓` なので追加説明不要との判断は妥当。

### Nits
- `crates/tui/src/app.rs:474-477` のコメントは "cache_creation stays in (it is full price on this request)" と簡潔で分かりやすい。protocol 側の doc と相互参照しなくても閉じて読めるので維持で良い。

## 判断
**Approve（完了可）** — ticket の要件・完了条件はすべて満たされ、互換性とアーキテクチャ境界を保ちながら最小差分で意味付けを修正している。リネームを伴うフィールド名変更も意味のドリフトを防ぐ正しい判断。
