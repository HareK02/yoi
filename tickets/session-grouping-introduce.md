# session-store: Session（Segment 群の grouping）導入

## 背景

`segment-rename` で物理 append-only 単位を `Segment` に揃えた。続けて、ユーザー視点の「同じ会話の家系」を表す **`Session`** を導入する。

`Session` は fork tree 全体を 1 つにまとめる grouping で、compaction / fork は同 Session 内に新 Segment を生やす操作になる。これにより:

- Session 数が fork で爆発せず、`WHERE session_id = ?` だけで fork tree 全体が取れる。
- resume の指定は `(SessionId, SegmentId)` の組で行える。
- pod-persistent-state 側で Pod ↔ Session の対応関係を扱いやすくなる。

## 要件

- 新 `SessionId` 型を `session-store` に追加。Segment は `parent_session_id` を持つ。
- Segment 生成パスでの session_id 決定:
  - new session: 新 `SessionId` を発行
  - compaction: 元 Segment と同 `SessionId` を継承
  - fork (live auto / 過去 fork いずれも): 元 Segment と同 `SessionId` を継承
- `SessionOrigin` を以下のいずれかに整理:
  - `compacted_from { segment_id, at_turn_index }`
  - `forked_from { segment_id, at_turn_index }`
  - どちらも同 Session 内 segment への参照であることを型レベルで保証。
- restore API を `(SessionId, SegmentId)` を取る形に。`SegmentId` のみを取る既存経路は内部で `SessionId` を解決する shim を一段噛ませる。
- FsStore layout に Session 単位の index を追加（Session → Segment 列挙が `WHERE session_id = ?` 相当で引けること）。形式は `<data_dir>/sessions/<session_id>/<segment_id>.jsonl` または別ファイル index、実装時に決定。
- Session 内の leaf Segment 列挙 API を提供（pod-name resume 等から使う）。

## 完了条件

- `SessionId` 型と Session 単位 metadata の永続表現が決まり、FsStore で読み書きできる。
- compaction / fork が同 Session 内 Segment 増分として記録される。
- `(SessionId, SegmentId)` での restore が動作し、leaf 以外を指定した read-only restore も実装上は可能。
- 既存 session を Session 単位に grouping する migration 戦略が決まっている（プロジェクト方針として後方互換は作らないため、既存 sessions ディレクトリは破棄して良いかどうかをここで明示）。
- `cargo check --workspace` および全テストが通る。

## 範囲外

- live auto-fork の marker 形式（別チケット `live-fork-marker`）。
- Pod 単位 metadata（Phase 2 一連のチケット）。
- TUI からの Session/Segment 選択 UI。
- DB backend 実装。

## 関連

- `tickets/segment-rename.md` (前提)
- `tickets/live-fork-marker.md`
- `tickets/pod-state-backend.md`
- `crates/session-store/`

## Review

- 状態: Approve with follow-up
- レビュー詳細: [./session-grouping-introduce.review.md](./session-grouping-introduce.review.md)
- 日付: 2026-05-20
