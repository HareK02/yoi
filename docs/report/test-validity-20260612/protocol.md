# テスト妥当性レビュー: protocol

- 評価: 概ね良い

## 確認範囲

- 対象は `crates/protocol` と解釈した。
- 読んだ範囲:
  - `crates/protocol/README.md`
  - `crates/protocol/Cargo.toml`
  - `crates/protocol/src/lib.rs`
  - `crates/protocol/src/stream.rs`
- 実行した検証:
  - `cargo test -p protocol` — pass。unit tests 38 件、doc tests 0 件。

## 現在のテストがよくカバーしていること

`protocol` crate の主責務である「Pod client/server 間の wire protocol 型」をかなり広く検証できている。特に以下は妥当。

- `Method` / `Event` の serde tag 形式を、`serde_json::Value` または固定 JSON で確認している。
- `Method::Run` と `Segment::{Text, Paste}` の roundtrip がある。
- 未知の `Segment` kind が `Segment::Unknown` に落ちる forward compatibility 要件を直接テストしている。
- `Notify`, `PodEvent`, completion, snapshot, status, compaction, tool result, LLM retry/continuation など、最近の運用上重要そうな wire shape が個別に押さえられている。
- `Snapshot` の legacy default (`status`, `context_window`, `context_tokens`) を確認しており、互換性テストとして価値がある。
- `PodEvent::should_notify_agent()` のような単なる serde ではない分類 invariant もテストされている。

この crate はロジックが薄く、ほぼ protocol schema の authority なので、現在のテストの中心が serde shape / roundtrip になっていること自体は適切。

## 不足 / 疑問のあるテスト

- `src/stream.rs` の `JsonLineReader` / `JsonLineWriter` にテストがない。JSONL reader/writer はこの crate の README 上の「JSONL message boundary」に近い責務なので、空行 skip、EOF、invalid JSON の `InvalidData`、writer の newline/flush 相当は最低限押さえたい。
- `Method` / `Event` の variant coverage は完全ではない。
  - 未確認の例: `Method::Cancel`, `ListRewindTargets`, `RewindTo`, `Shutdown`; `Event::TextDone`, `ToolCallStart`, `ToolCallArgsDelta`, `ToolCallDone`, `Usage`, `Shutdown`, `MemoryWorker`, `RewindTargets`, `RewindApplied` など。
  - protocol crate は variant 追加・rename の影響が大きいため、未テスト variant が増えると wire contract の退行を見逃しやすい。
- `Segment::flatten_to_text()` と `Method::run_text()` が未テスト。特に `flatten_to_text()` は sigil 復元・貼り付け content・unknown placeholder という明確な仕様を持つので、serde よりも壊れやすい実装ロジックとしてテスト価値が高い。
- `ScopeRule` / `Permission` の wire shape と `recursive` default は `ScopeSubDelegated` 経由で一部触れているが、`recursive` 省略時の default や `Permission` の lowercase serde は直接確認されていない。
- 一部テストは roundtrip のみで、期待 JSON 名や payload 値の確認が弱い。
  - 例: `pod_discovery_methods_roundtrip()` は `RestorePod { name }` / `RegisterPeer { name }` の値を assert していない。
  - roundtrip だけだと、同一型内で serialize/deserialize が揃って壊れた場合に検出できないことがある。
- 固定 JSON 文字列との完全一致テストが一部ある。wire format の固定には有効だが、serde の field order に依存するため、意図しない脆さもある。重要な canonical shape だけに絞り、他は `serde_json::Value` で意味を assert する現在の混合方針は概ね妥当。

## 追加提案

- `stream.rs` に async unit tests を追加する。
  - writer が JSON + `\n` を出す。
  - reader が空行を skip して次の JSON を読む。
  - EOF で `Ok(None)`。
  - invalid JSON が `io::ErrorKind::InvalidData` になる。
  - writer → reader の小さな end-to-end roundtrip。
- `Segment::flatten_to_text()` の仕様テストを追加する。
  - `Text`, `Paste`, `FileRef`, `KnowledgeRef`, `WorkflowInvoke`, `Unknown` を混ぜた順序保持。
  - `@`, `#`, `/` の sigil 復元。
- 未カバーの control/event variant に、少なくとも代表的な pinned wire-shape テストを足す。
  - `Cancel`, `Shutdown`, `RewindTo`, `ListRewindTargets`
  - `ToolCall*`, `Usage`, `TextDone`, `RewindTargets`, `RewindApplied`, `MemoryWorker`
- roundtrip だけのテストは、重要 payload を assert する。
  - `RestorePod.name`
  - `RegisterPeer.name`
  - `PodsListed` / `PodRestored` / `PeerRegistered` は現在 payload equality があるので良い。
- `ScopeRule` の default と permission rename を単体で確認する。
  - `{"target": "...", "permission": "read"}` で `recursive == true`
  - `Permission::Write` が `"write"` になる。

## 実行したコマンド

```sh
cd /home/hare/Projects/yoi && cargo test -p protocol
```

結果: pass。38 unit tests passed、0 failed。doc tests は 0 件。

