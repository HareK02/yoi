# session-store: SessionId → SegmentId へのリネーム

## 背景

永続化層の現 `SessionId` は、append-only log の物理的 / 復元上の単位を指している（compaction や fork で新 ID が発行される）。一方ユーザー視点では「同じ会話の継続」が compaction / fork を跨いで成立しており、ここに**ユーザー視点の会話の家系 = `Session`** と **物理 append-only 単位 = `Segment`** の 2 階層を導入したい。

本チケットは 2 階層導入のうち、**先に物理単位側のリネームだけ**を機械的に済ませる。新 `SessionId`（grouping 概念）は次チケットで導入する。

## 要件

- `crates/session-store/` 内の `SessionId` 型・関連関数 (`SessionLogWriter` / `Store::append` / `fork` / `fork_at` 等) を `SegmentId` に統一。
- 同様に `pod` / `pod-registry` / `pod-cli` / `llm-worker` 関連の呼び出し元の参照を `SegmentId` に追従。
- `SessionLogWriter` / `SessionStart` / `SessionOrigin` 等の `Session` プレフィックス型のうち、**segment レベルを指しているもの**は同時に `Segment` プレフィックスへ変える。
  - 線引きの基準: そのデータが「fork / compaction で新規生成される log 1 本」に紐づくなら `Segment`。会話の家系全体に紐づく概念は何も無いはずなので（次チケットで初めて出てくる）、本チケット内では全て Segment 系に倒す。
- crate 名 (`session-store`) の rename 要否は本チケットでは扱わず保留。中身が Segment 中心になった事実のみで判断材料を残す。
- `--session <UUID>` 系 CLI 引数は内部実装としては `SegmentId` を受けるが、外部表記は変更しない（ユーザー向け ID 体系の整理は `session-grouping-introduce` で扱う）。

## 完了条件

- `SessionId` 型がコードベースに残らない（後続の Session grouping で導入する新 `SessionId` は無関係）。
- `cargo check --workspace` および全テストが通る。
- 既存 session の JSONL を Segment 中心の API で読み書きできる。

## 範囲外

- 新 `SessionId` (Segment 群の grouping) の導入（次チケット `session-grouping-introduce`）。
- session-store crate 名の rename。
- 外部公開 CLI 引数の rename。

## 関連

- `tickets/entry-hash-abolish.md` (前提)
- `tickets/session-grouping-introduce.md` (後続)
- `crates/session-store/`
- `crates/pod/src/pod.rs`
