# Event / LogEntry: System 注入経路を SystemItem 一本に統合する

## 背景

エージェントシステム (= ユーザー由来でも LLM 由来でもない、Pod 自身) が LLM context に注入する `role:system` の `Item::Message` は、現状 3 系統の ad-hoc 経路で並走している:

1. **`Method::Notify`** — 外部からの非同期メッセージ
   - controller → `Event::Notify { message }` (生 message echo)
   - `pod.push_notify(message)` → `NotifyBuffer` → `pending_history_appends` で `[Notification] <msg>` の system_message として history に commit
2. **`Method::PodEvent`** — 子 pod のライフサイクル通知
   - controller → `Event::PodEvent(event)` (typed echo)
   - `render_event` で 1 行整形 → `NotifyBuffer` (Notify と合流) → 同じく `[Notification] <rendered>` として commit
3. **Interceptor 内部注入** — `@<path>` / `#<slug>` / `/<slug>` の解決結果
   - `PodInterceptor::on_prompt_submit` の `ContinueWith` で `[File: <path>]` / `[Knowledge: <slug>]` / workflow 本文の system_message を history に append
   - wire echo は無し

これらは全部 「**人でも LLM でもなく、エージェントシステムが LLM に与えた情報**」 という同一カテゴリで、history への commit 形 (`role:system` の `Item::Message`) もほぼ同じだが、wire event 側は echo/typed/未送信が混在し、TUI 側のブロックも `Block::Notify` / `Block::PodEvent` / `Block::SystemMessage` の 3 つに分かれている。

加えて `LogEntry::HookInjectedItems` という命名が誤称: 実際に注入しているのは公開 `Hook` ではなく **`Interceptor`** で、内部機構専用の経路。`hook.rs` モジュール doc でも 「Hook は read-only な公開 extension surface」 「内部機構は Interceptor を使う」 と明確に分離されている。

このばらつきの結果:
- wire 上、同じ通知が `Event::Notify` (生) + `Event::HookInjectedItems` (整形版) の 2 重に流れて TUI が重複描画した (`pod-state-from-session-log` 改修中に表面化)
- kind 判別がテキストプレフィックス (`[Notification] ...` / `[File: ...]`) 頼みで脆い
- 新しい注入種 (`<system-reminder>` 等) を足すたびに 1 系統増える設計圧力
- `Method::Notify` の "Notify" 語感が view-only な `Alerter` (本来 "Notification" 寄り) とぶつかっている

LLM は `role:system` を生成しないため、worker.history 中の `role:system` 項目は構造的にすべてこのエージェント注入経路に由来する。この性質を型として表に出す。

## 方針

`Tool` パターンに倣って 「**1 つの concept + kind ベース dispatch**」 に統合する。

- wire event は 1 種類: `Event::SystemItem { kind, payload }` (1 件ずつライブ配信)
- LogEntry は kind 揃いで batch する単一バリアントに置き換え、`Hook` 命名を捨てる: `LogEntry::SystemItems { ts, items: Vec<SystemItem> }`
- Pod 内部の注入路 (NotifyBuffer / `format_notify` / `render_event` / Interceptor.ContinueWith) は **全部「kind 付き `SystemItem` を作って worker.history に commit」 という単一形式に合流**
- TUI は kind 別に Block を出し分け (現 `ToolCallBlock` がツール別に見た目を出すのと同じ構造)

単数/複数の使い分けは既存パターンに揃える:
- 1 件単位の wire event は `Event::SystemItem` (`Event::TextDelta` と同じ呼吸)
- 永続バッチは `LogEntry::SystemItems` で `Vec<SystemItem>` を内包 (`LogEntry::AssistantItems` / `ToolResults` と同じ呼吸)

`Method::Notify` / `Method::PodEvent` は外部 API としてはそのまま残す (入口の意味付けは別)。 中で `SystemItem::Notification` / `SystemItem::PodEvent` に変換されて以後は単一経路、という整理。

`Event::Alert` (= LLM context に乗らない純 UI 通知) は **別経路として明確に残す**。 view-only な persistent stream (Alerter の subscribe_with_snapshot) としてすでに正しく機能している。 "Notification" 語感の衝突は、本チケットで context 注入側を `SystemItem` に rename することで解消する (Notification は `SystemItem` の一 kind に格下げ、`Alerter` が "Notification" 語感の本来のオーナーに戻る)。

## 要件

- wire event は 1 種類: `Event::SystemItem { kind, payload }` で全注入が乗る。 `Event::Notify` / `Event::PodEvent` / `Event::HookInjectedItems` は protocol から削除
- LogEntry は `HookInjectedItems` を rename + items を kind 付き typed shape に置換。 新名 `LogEntry::SystemItems { ts, items: Vec<SystemItem> }` で wire tag は `system_items`
- `SystemItem` の kind 列挙は最低限以下を含む:
  - `Notification { message }` (`Method::Notify` 由来)
  - `PodEvent { event: PodEvent }` (子 pod ライフサイクル)
  - `FileAttachment { path, content }` (`@<path>` 解決)
  - `Knowledge { slug, body }` (`#<slug>` 解決)
  - `Workflow { slug, body }` (`/<slug>` 解決)
  - 将来追加可能 (`Reminder` 等) を見越した拡張点
- Pod 側の `NotifyBuffer` / `format_notify` / `render_event` / `Interceptor::on_prompt_submit ContinueWith` は `SystemItem` を中間表現として通る。 worker.history への append は最終的に `Item::system_message` + 対応する `SystemItem` 1 件を `LogEntry::SystemItems` として commit
- TUI は `Event::SystemItem` を kind で dispatch して描画する。 既存 `Block::Notify` / `Block::PodEvent` / `Block::SystemMessage` を `Block::SystemItem(SystemItemBlock)` に集約 (or 既存 Block を再利用しつつ駆動イベントだけ統一)
- `Method::Notify` / `Method::PodEvent` (外部入口 API) は名前を維持し、内部で `SystemItem::Notification` / `SystemItem::PodEvent` に変換される
- `Event::Alert` / `Alerter` は無変更

## 完了条件

- `Event::Notify` / `Event::PodEvent` / `Event::HookInjectedItems` が protocol から削除されている
- `LogEntry::HookInjectedItems` が削除され、`LogEntry::SystemItems` に置き換わっている (旧 wire tag を deserialize alias で残すかは実装判断)
- TUI が `Event::SystemItem` 駆動で system 系ブロックを構築している。 ライブ通知の二重描画が起きない
- `Method::Notify` と `Method::PodEvent` は外部 API としては変わらず動く
- `Event::Alert` / `Alerter` 経路は無変更

## 範囲外

- `Method::Notify` / `Method::PodEvent` の rename (入口名の整理は別の話)
- `Event::Alert` / `Alerter` 系の変更
- 旧 session log (`hook_injected_items` を含む) のファイル変換: deserialize alias で読めるところまでで、ファイル書き換えは行わない
- TUI 内の `Block::SystemItem` 詳細な視覚設計

## 関連

- 前提となる `tickets/pod-state-from-session-log.md` (state 正本を session log に統合) の後続。 同チケット内で `Event::HookInjectedItems` を導入したが、 直後に「Hook 命名は誤り」「Notify/PodEvent と二重」と判明したため本チケットで整理する
- CLAUDE.md の 「context に乗せる前に history に commit する」 加工原則に整合する整理 (現実装の経路を統一形にするだけで、原則自体は変わらない)
