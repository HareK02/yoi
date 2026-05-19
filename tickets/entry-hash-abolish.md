# session-store: Entry hash chain の廃止

## 背景

session-store の各 entry は SHA-256 hash chain (`prev_hash` → `hash`) で連結されており、`HashedEntry` として JSONL に 1 行ずつ書かれる。実際に効いている用途は以下の 2 つだけ:

1. `ensure_head_or_fork` (`crates/pod/src/pod.rs:1591`) — store 末尾と Pod 保持の `head_hash` を比較して auto-fork。**末尾識別子があれば良い**。
2. `fork_at(source_id, at_hash)` (`crates/session-store/src/session.rs:425`) — 過去 entry からの fork。`pod-session-fork` の入口仕様は turn 番号であり、entry hash は内部 pointer に過ぎず turn boundary (TurnEnd entry の index) で代替可能。

宣伝されている改竄検知 (tamper-evident chain) は walk して verify するルートがコード上に存在せず、削除しても regression にならない。

write 経路は既に sync 化済み (`6e5b148`)。前提足場は揃っている。

## 要件

- `HashedEntry` 廃止、JSONL は 1 行 1 `LogEntry`。
- `compute_hash` / `build_chain` / `EntryHash` の撤去（外部に公開している場合は呼び出し元を含めて）。
- `SessionOrigin.at_hash` → `at_turn_index` (TurnEnd entry 由来の turn 番号) に置換。
- `ensure_head_or_fork` の検知ロジックを末尾 seq 比較ベースに置換。形式は実装時に決める（terminal marker entry / 末尾 seq の何れか）。
- **`session_head` mutex の撤去**。hash chain が無くなる結果として "head_hash を直前 entry から取得して次へ渡す" という serialize 必須の依存が消える。1 行 < `PIPE_BUF` (Linux 4KB) の `O_APPEND` write は kernel が atomic に直列化するため user space で mutex 不要。
- 既存 JSONL の読み込み互換は不要（プロジェクト方針として後方互換性は作らない）。

## 完了条件

- `HashedEntry` / `prev_hash` / `compute_hash` / `build_chain` / `EntryHash` がコードベースから消えている。
- `SessionOrigin` が `at_turn_index` を保持し、`fork_at` も同 API になっている。
- `session_head` mutex への参照が無く、`SessionLogWriter` 系は writer ハンドルを `Arc` で持つだけになっている。
- `cargo check --workspace` および `cargo test -p session-store -p pod` が通る。
- 既存 session を新規再生成して JSONL が 1 行 1 `LogEntry` になっていることを目視確認できる。

## 範囲外

- `SessionId` → `SegmentId` のリネーム（別チケット `segment-rename`）。
- 新 `SessionId` (Segment 群の grouping) 導入（別チケット `session-grouping-introduce`）。
- live auto-fork の marker 形式の最終決定（別チケット `live-fork-marker`、ここでは末尾 seq 比較相当の最小実装で繋ぐ）。

## 関連

- `crates/session-store/src/session_log.rs`
- `crates/session-store/src/session.rs`
- `crates/pod/src/pod.rs:1591` `ensure_head_or_fork` 経路
- `tickets/segment-rename.md` (後続)
