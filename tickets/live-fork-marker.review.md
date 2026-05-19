# Review: session-store live auto-fork の marker 形式確定

対象コミット: `ac3ee5f feat: live auto-fork の marker 形式を確定（seq 比較 + forked_from 記録）`
ベース: `develop` (`46b0e20`)

## 前提・要件の確認

- **marker 形式を (a)/(b) のどちらかに確定して実装**: 達成。(b) 末尾 entry-count 比較を本決定として採用。`entry-hash-abolish` で入った最小実装 (`store_count == *entries_written` 比較) をそのまま昇格し、`segment.rs:151-153` / `pod.rs:1666-1673` で検知。terminal marker variant は追加していない。
- **(b) の欠点「fork 関係を引くには別 metadata index が要る」の解消**: 達成かつ妥当（下記アーキ節）。新 Segment 側 `SegmentStart.forked_from` への前向き記録で log だけから lineage を辿れるようにし、別 index を不要にした (`segment.rs:163-166`, `pod.rs:1689-1692`)。
- **過去 fork は元 Segment を touch しない invariant の明文化**: 達成。`fork_at` doc に `# Invariant: the source segment is never mutated` を追記 (`segment.rs:456-462`)。`fork_at` 本体は `read_all` → `collect_state` → `create_segment` のみで source を書かないことをコードで確認 (`segment.rs:469-499`)。
- **`pod.rs` の auto-fork 経路を確定形式に合わせる**: 達成。本番経路 `Pod::ensure_segment_head` の inline auto-fork に `forked_from` を記録 (`pod.rs:1683-1694`)。

## 完了条件

- **live auto-fork の marker 形式が決まり実装に反映**: OK（上記）。
- **nested 過去 fork が正しく動く（test）**: OK。`session_test::nested_past_fork_leaves_ancestors_immutable` が root→fork1→fork2 を作り、(1) root と fork1 が子の fork で不変、(2) fork2 の `forked_from.segment_id == fork1`（祖先 root ではなく直接の親）を検証。実行 pass 確認済み。
- **並行 writer auto-fork が検知され新 Segment に分岐（test）**: OK。本番経路を通す `compact_events_test::concurrent_writer_drift_auto_forks_with_forked_from` が、別 writer の append で drift→`ensure_segment_head` が新 Segment へ分岐→`forked_from` 記録→元 Segment は foreign append 以外 byte-for-byte 不変（entry 数 +1 のみ）を検証。実行 pass 確認済み。

## アーキテクチャ・スコープ

- **option (b) + forked_from 記録の設計判断**: 妥当。(a) terminal marker は元 Segment を mutate するため、過去 fork（不変）と live fork（可変）で semantics が割れ、チケット背景が懸念した「nested で marker 位置解釈が破綻」を構造的に抱える。(b) を採ったうえで欠点（lineage の引きにくさ）を新 Segment 側の前向き記録で潰したことで、`forked_from` / `compacted_from` の SegmentOrigin スキーマが既に存在する流れにも自然に乗っており、新型・新 variant を一切増やしていない。(a) を採るべき残存理由は無い。
- **scope**: 範囲外（過去 fork の物理コピー方式、fork tree 可視化 UI）に踏み込んでいない。terminal marker variant 非追加も option(a) 不採用と整合し過不足なし。最小変更に収まっている。
- **at_turn_index の意味論**: 揃っている。`SegmentOrigin.at_turn_index` は「直近 TurnEnd の turn_count」と定義 (`segment_log.rs:174-184`)。auto-fork は `w.turn_count()` を渡す (`pod.rs:1691`) が、`ensure_segment_head` は `prepare_for_run` 内で**新ターン実行前**に呼ばれる (`pod.rs:1175`) ため、`turn_count()` は直近完了ターンの値=分岐点を指す。これは compaction が `compacted_from.at_turn_index` に渡す値 (`pod.rs:2193 source_turn_count = worker.turn_count()`) および `TurnEnd` が記録する値 (`pod.rs:1880`) と同一ソースで、3 経路で一貫している。テストの `at_turn_index == 0`（0 ターン worker）も定義どおり。
- **forked_from 記録による既存経路への副作用**: 無し。`collect_state` の `SegmentStart` アームは `forked_from` / `compacted_from` を `..` で破棄しており replay には一切影響しない (`segment_log.rs:251-262`)。`forked_from` の読み手は本 diff 時点で存在せず（grep 済み）、restore / compaction / collect_state は SegmentStart の他フィールドのみ使用。`#[serde(default, skip_serializing_if=...)]` 付きなので旧ログ互換も維持。

## 指摘事項

### Non-blocking / Follow-up

- **auto-fork ロジックの二重実装** — `session_store::ensure_head_or_fork`（free fn, `segment.rs:143`、本番未使用でテスト専用）と `Pod::ensure_segment_head`（本番, inline `pod.rs:1666-1707`）に同じ fork ロジックが二重に存在し、今回両方へ `forked_from` 記録を入れた。この二重化は `entry-hash-abolish` 時点からの既存事項で、当時の review (`6e791d8:entry-hash-abolish.review.md:13`) でも両者を別々に更新したことが記録されている既知の状態。drift リスクはあるが、(1) 両者がそれぞれテストされている（free fn = `session_auto_forks_on_conflict`、inline = `concurrent_writer_drift_auto_forks_with_forked_from`）、(2) unify は本チケットのスコープ（marker 形式の確定）外、という理由で本チケットでの blocking とはしない。ただし free fn が本番 0 caller のままテスト専用で残り、本番は inline を持つ構造は、放置すると確実に drift するため、KNOWN_ISSUES への明示登録または近接チケットでの一本化（Pod が free fn を呼ぶ形へ寄せる）を推奨。

### Nits

- `concurrent_writer_drift_auto_forks_with_forked_from` の inline manifest 文字列内コメント "test-pod" 等は問題なし。auto-fork が compaction と独立して発火することを保証するため `NO_COMPACT_MANIFEST_TOML` を用意した意図はコメントに明記されており妥当。

## 判断

**Approve（完了可）** — 要件・完了条件をすべて充足し、option(b)+forked_from 前向き記録という設計判断は妥当で、新型・新 variant を増やさず最小変更に収まっている。replay / restore / compaction への副作用も無い。唯一の懸念である auto-fork ロジック二重化は本チケット起源ではない既知の non-blocking 事項で、両経路ともテスト済み。フォローアップとして二重実装の一本化（または KNOWN_ISSUES 登録）のみ推奨。
