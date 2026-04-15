# レビュー: TUI 通知チャネル

対象差分: `crates/pod/src/{notifier,pod,controller,agents_md,lib,socket_server}.rs`, `crates/llm-worker/src/worker.rs`, `crates/protocol/src/lib.rs`, `crates/tui/src/{app,ui}.rs`（いずれも未コミット）

## 要件達成状況

| 要件 | 状態 |
|---|---|
| Pod 層が型付き通知を発行する API を持つ | ✅ `Notifier::notify(level, source, message)`、`PodHandle::notify` / `Pod::notify` ラッパ |
| `tracing` とは別系統 | ✅ 既存 `tracing::warn!` は並存、通知は `Notifier` 経由のみ |
| 構造化型（level / source / message / timestamp） | ✅ `protocol::Notification` に全4項目。timestamp は unix ms の i64 |
| レベルは Warn / Error（Info は設計判断） | ✅ Info は除外（設計判断として妥当） |
| 発生源の列挙（Pod / Worker / Compactor / Tool 境界等） | ✅ `NotificationSource::{Pod, Worker, Compactor, AgentsMd}`。Tool 境界は Worker に集約 |
| TUI が受信して表示する | 🟡 表示はする（`MessageKind::NoticeWarn/Error` + 色 + bold）が、**ticket が想定した「一時表示・通知ペイン」レベルには到達していない**。後述 |
| Error と Warn の視覚的区別 | ✅ Yellow bold / Red bold + `[notice]` / `[notice error]` prefix |
| 通知履歴が見られる | 🟡 `output_queue` → scrollback に残るのみ。専用ペインでの閲覧は無い |
| 新着通知の一時表示 | 🟡 普通のメッセージ行として積まれるだけ、トースト / ステータスバー常駐は無し |
| 複数通知の重ね合わせ | 🟡 単純 append、特段の配慮は無し |
| 既存 `tracing::warn!` の置換（compaction 失敗） | ✅ `pod.rs` の mid-run / post-run 両経路で notify。並行して tracing も残る |
| 既存 `tracing::warn!` の置換（ツール出力切り詰め） | ✅ `worker.rs` が `on_warning` コールバック経由で Controller → Notifier に流す |
| 既存 `tracing::warn!` の置換（AGENTS.md） | ✅ `read_agents_md` が `AgentsMdResult { body, warnings }` を返し、`pod.rs` で notify |

**コア要件は達成**。「TUI の表示方法」は設計議論を含む項目で、ticket の "設計で決めること" に「トースト / ステータスバー / 通知ペイン / その組合せ」と列挙されていた部分を、実装者は**「履歴内の区別された行」に絞った**と読める。これは最小実装としては筋が通るが、ticket 本文の「見落としにくい位置に一時的に表示」という表現とは**乖離**があり、判断を user に返すべき。

## アーキテクチャ統合

### 良い点

- **`Notifier` の race-free 購読**: `subscribe_with_snapshot` が `buffer` の mutex を保持したまま `event_tx.subscribe()` を呼び、snapshot をクローンしてから返すことで、「通知が snapshot と live の両方に現れる」「どちらからも漏れる」の両方を同時に防いでいる。`notifier.rs` のテスト `subscribe_snapshot_and_live_do_not_overlap` で設計意図を lock-in している。late subscriber 対応は本チケットの肝であり、最もよく出来ている部分。
- **層の分離**: Worker は `on_warning(Box<dyn Fn(&str) + Send + Sync>)` というタイプ消去されたコールバックを受ける形にし、`protocol::Notification*` 型に依存していない。Controller 側で closure に notifier をキャプチャして橋渡しする。Worker が protocol に依存せずに済んでおり、llm-worker は低レベル基盤のままという方針（memory）とも整合。
- **`read_agents_md` の pure 分離**: 以前は `Option<String>` + 直接 `tracing::warn!` だったが、`AgentsMdResult { body, warnings }` を返す形に変更され、呼び出し側が副作用（notify）を担当する。pure function + 副作用集約の分離として綺麗。
- **既存 broadcast channel への相乗り**: `Event::Notification(Notification)` という新バリアントとして既存の `broadcast::Sender<Event>` に載せたことで、新しい通信経路を作らずに済んでいる。`socket_server` 側の配線変更も `subscribe_with_snapshot` の snapshot を prelude として書き出し、以降は既存 loop に合流する、という自然な形。
- **`Pod::attach_notifier` パターン**: Controller が Pod の construction 後に notifier を差し込む（`Pod::new` 自体は notifier を持たない）ため、Pod 直接 new の tests でも Notifier が不要。`None` のときは `Pod::notify` が no-op になる。

### 懸念 1: 🟡 `Notifier::buffer` が無制限に伸びる

```rust
struct Inner {
    event_tx: broadcast::Sender<Event>,
    buffer: Mutex<Vec<Notification>>,
}
```

セッションが長寿命かつ通知が頻発するケース（compaction 失敗がループするとか、ツール出力切り詰めが毎ターン起きるとか）で、`buffer` は単調増加する。新規クライアントが接続した瞬間に `subscribe_with_snapshot` が全部クローンするので、**1 接続あたりのメモリ消費と初期送信コストも比例して増える**。

ticket には明示的な上限要件は無いが、実運用での**メモリ健全性**として上限を設けるべき。たとえば:

- 上限件数 (例: 256 件) でリング化（最古から落とす）
- 時間窓（直近 N 分）
- 落とした件数だけ "[N older notifications elided]" の synthetic notification を先頭に置く

**判断**: 要検討。後続チケットとして切るか、本チケットの範囲で追補するか。

### 懸念 2: 🟡 TUI 側の表示が ticket 要件の minimum interpretation

ticket 要件:
> - 新着通知はユーザーが見落としにくい位置に**一時的**に表示される（トースト / ステータスバー等）
> - 履歴が見られる（キーバインドで通知ペインを開ける等）

実装:
- `MessageKind::NoticeWarn/Error` として通常のメッセージ行と同じ `output_queue` に積む
- Yellow/Red の bold + `[notice]` prefix で視覚的に区別
- トースト・ステータスバー常駐・通知ペインは無し

ユーザーが会話に集中していてスクロールが別位置にあった場合、新着通知は画面外に流れて気付けない可能性が高い。

一方、ticket の "設計で決めること" には 4 択が並んでおり、実装者が minimum viable として「履歴に色付きで積む」を選んだという解釈もできる。

**判断**: user が「これで良い」と言えば OK。「やはり見落とす」となれば別タスクとして追加実装（status 行 1 段拡張でラスト通知を pin する、など）が必要。

### 懸念 3: 🟢 `NotificationSource` に Pod 名が入らない

現状 `NotificationSource` はカテゴリだけ持ち、Pod 名や具体的な tool 名は `message` 本文に埋め込む設計。単一 Pod 前提ではこれで十分。将来 `tickets/native-gui-mvp.md` で GUI が複数 Pod を spawn するようになると「どの Pod の通知か」がメッセージパースでしか分からなくなる。

**判断**: 不問。複数 Pod 対応のチケット（将来）で `Notification` に Pod 名フィールドを追加するのが自然。現時点で先回りする必要は無い。

### 懸念 4: 🟢 `PodHandle::subscribe` の用途

以前は `handle.subscribe()` を直接呼んで broadcast::Receiver を得ていたところが、今は `handle.notifier.subscribe_with_snapshot()` 経由になった。`PodHandle::subscribe` 自体はまだ存在しており、socket_server 以外に呼び出し元があるかは未確認。無ければ将来削除可能。

**判断**: 不問。クリーンアップは別途。

## 完了条件照合

- [x] Pod 層が型付き通知を発行する API を持つ
- [x] TUI がその通知を受信して表示する
- [~] 履歴保存 — 一応 scrollback / `Notifier::buffer` には残るが、ticket 本文の「セッション単位で履歴を見られる」は minimal な達成
- [~] 手動テストで TUI 画面上に現れることの確認 — コードレベルでは通るが、実機確認の記述は無い（これは受け入れテストの話）

## テスト

- `notifier.rs` の 3 ケース（broadcast / late subscriber snapshot / snapshot-live non-overlap）は設計要点を的確にカバー
- `protocol/lib.rs` で `event_notification_format` が JSON 表現を lock-in
- `agents_md.rs` のテストは `AgentsMdResult` 変更に追従し、`non_utf8_surfaces_warning` が新規に追加されて warning 経路をカバー
- Controller / Pod / socket_server レベルの integration test は無し。`Notifier` の単体テストで十分と割り切った判断と見える

## 結論

**コア要件は達成、アーキテクチャは筋が良い**。特に race-free subscribe と層の分離は見事。

残る指摘:

1. 🟡 **buffer 無制限**: 実運用でメモリ単調増加の可能性。上限の検討要
2. 🟡 **TUI 表示の強度**: ticket 文面からは「通知は見落とされないべき」と読めるが、実装は履歴行に紛れる最小形。user 判断待ち
3. 🟢 **Pod 名フィールド** / 🟢 **`PodHandle::subscribe` の残存**: 不問

**受け入れ可否**: 指摘1・2について user 判断。採否次第で「このまま受け入れ」「buffer 上限だけ追補」「TUI 表示も強化」の 3 パスに分岐する。
