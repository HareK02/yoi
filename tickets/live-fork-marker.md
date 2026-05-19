# session-store: live auto-fork の marker 形式確定

## 背景

`entry-hash-abolish` で `ensure_head_or_fork` は末尾 seq 比較ベースに置換されたが、最小実装で繋いだだけで marker 形式の本決定は保留している。

live auto-fork（concurrent writer 検知）と過去 fork（UI から turn 選択）は性質が違う:

- **live auto-fork**: 元 Segment の末尾に terminal marker (例: `Forked { to: SegmentId }`) を append する CoW semantics。以降の writer は marker を見て新 Segment に自動移動。
- **過去 fork**: 元 Segment は無変更で、replay して新 Segment を生やすだけ。

両者を同じ marker で扱うと、過去 fork から更に過去で fork した場合に元 Segment への marker 位置解釈が複雑化して破綻する。**過去 fork は元 Segment に触れない方針を固定**した上で、live auto-fork 側の marker 形式を確定する。

## 要件

- live auto-fork 検知の形式を以下のどちらかに確定して実装:
  - (a) **terminal marker entry**: 元 Segment 末尾に `Forked { to: SegmentId }` 等の LogEntry を 1 行 append する
  - (b) **末尾 seq 比較**: 元 Segment に書き込みは行わず、writer の保持する末尾 seq と store の末尾 seq の差分のみで検知する
- 選択基準:
  - (a) は読み手側が fork チェーンを log だけから辿れる利点。書き手競合時に 1 write 増えるコスト。
  - (b) は元 Segment が完全 immutable で、過去 fork との semantics 統一が綺麗。fork 関係を引くには別の metadata index が要る。
- 過去 fork 側は引き続き元 Segment を touch しないことを invariant として明文化。
- `pod.rs` の `ensure_head_or_fork` を確定後の形式に合わせて書き直す。

## 完了条件

- live auto-fork の marker 形式が決まり、実装に反映されている。
- 過去 fork からの nested 過去 fork が正しく動く（test で確認）。
- 並行 writer による live auto-fork が正しく検知され、新 Segment に分岐する（test で確認）。

## 範囲外

- 過去 fork の物理コピー方式（既に `fork_at` で `SessionStart` seed 1 回書き込みの方針で固定）。
- Fork tree の可視化 UI。

## 関連

- `tickets/entry-hash-abolish.md` (前提)
- `tickets/session-grouping-introduce.md` (前提、Session 単位の lineage と整合)
- `tickets/pod-session-fork.md`

## Review

- 状態: Approve（完了可）
- レビュー詳細: [./live-fork-marker.review.md](./live-fork-marker.review.md)
- 日付: 2026-05-20
- フォローアップ: auto-fork ロジックの二重実装（`ensure_head_or_fork` free fn と `Pod::ensure_segment_head` inline）の一本化、または KNOWN_ISSUES 登録を推奨（本チケット範囲外・両経路テスト済み）。
