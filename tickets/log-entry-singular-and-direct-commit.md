# Log writer を sync 化 + LogEntry を単数化

## 背景

`tickets/system-item-unify.md` で `Event::SystemItem` / `LogEntry::SystemItems` を導入し、 Notify / PodEvent / 各 ref 解決を 1 経路に統合した。 ただし実装の過程で 2 つの設計上の歪みが残った:

**1. `LogEntry::SystemItems { items: Vec<SystemItem> }` の複数形は実需要が無い**

- 既存 `LogEntry::AssistantItems { items: Vec<LoggedItem> }` / `ToolResults { items: Vec<LoggedItem> }` の形に揃えて plural にしたが、 これは旧 `save_delta` の turn 末尾バッチ commit 時代の名残
- 現在は worker の `on_history_append` callback が per-item で発火し、 drain task が 1 件ずつ classify+commit する形に移行済み。 `items: Vec<_>` の中身は常に 1 件
- `SystemItems` も同様で、 interceptor が drain した N 件をまとめているだけで「グルーピング」 の意味はない (同 drain 機会に来ただけ)
- wire 側では IPC server が `items.into_iter().map(...)` で per-item の `Event::SystemItem` に fan-out しており、 LogEntry 上のバッチと wire 上の単発の非対称が発生している

**2. `LogCommand::SystemItems` 追加が二重設計**

- `LogCommand` は元来「worker の sync callback (`on_history_append`) を async commit に橋渡しする」 ための小さな ferry。 `Item + Flush` の 2 variant だけだった
- system items に typed kind 情報を運ぶために `LogCommand::SystemItems(Vec<SystemItem>)` を増やしたが、 interceptor は async コンテキストなので sync→async の橋渡しは要らない。 単に「SystemItem を直接 commit する経路」 として LogCommand を流用しているだけで、 `LogCommand` の元来の責務 (sync→async ferry) を曖昧にしている

**3. そもそも log 周りで async が必要だった理由はほぼ無い**

log 周りで async になっている箇所を棚卸しすると:

| 箇所 | 現在 async | 本当に async でないとダメか |
|------|-----------|---------------------------|
| `Store::append` (FsStore) | tokio::fs | ❌ `std::fs::OpenOptions` + `writeln!` で良い (1 行 append、 < 1KB、 < 1ms) |
| `Store::read_all` (restore 時) | tokio::fs::read_to_string | ❌ Pod 起動時 1 回、 hot path じゃない |
| `Store::read_head_hash` | tokio::fs | ❌ 同上 |
| `session_head` mutex | `tokio::sync::AsyncMutex` | ❌ async は不要、 sync mutex で十分 |
| `sink.publish` / `subscribe_with_snapshot` | sync (元から) | — |
| `broadcast::send` | sync (元から) | — |

つまり log subsystem を async にしていた本当の理由は **tokio::fs を default 選択した点だけ**。 hash chain 維持のためでも性能のためでもない。 そして sync callback が async commit を呼べないという mismatch を解消するために `LogCommand` ferry を作る必要があった。

log writer を sync にすればこの mismatch そのものが消え、 `LogCommand` / drain task / Flush バリアは丸ごと不要になる。

## 方針

**A. log writer を sync に切り替える**

- `Store::append` / `read_all` / `read_head_hash` を `std::fs` / `std::io` ベースの sync API に変更 (or sync 版を併設して段階移行)
- `session_head` を `tokio::sync::AsyncMutex` から `parking_lot::Mutex` (or `std::sync::Mutex`) に変更
- `SessionLogWriter::append_entry()` を sync 関数に。 disk write の間 caller の thread が ms 未満ブロックする (local fs append、 < 1KB、 実害なし)
- Pod の `commit_entry().await` は `commit_entry()` (await 無し) に
- worker の `on_history_append` callback が直接 `writer.append_entry(classified_entry)` を呼ぶ
- interceptor も `Arc<SessionLogWriter>` を持って直接呼ぶ

**B. `LogCommand` / drain task / Flush バリアの撤廃**

- `LogCommand` enum を削除
- `run_log_drain` 関数を削除
- mpsc channel (`log_cmd_tx` / `log_cmd_rx`) を削除
- `persist_turn` の Flush バリアを削除 (sync 書き込みなので順序は call 順で自然に決まる)
- worker callback で直接 sync 書き込みするので、 `Worker::on_history_append` 経由のフローはそのまま (sync closure)

**C. LogEntry のアシスタント / ツール / システム系を単数バリアントに揃える**

- 新名 (write side):
  - `LogEntry::AssistantItem { ts, item: LoggedItem }`
  - `LogEntry::ToolResult { ts, item: LoggedItem }`
  - `LogEntry::SystemItem { ts, item: SystemItem }`
- 旧 plural variant (`AssistantItems` / `ToolResults` / `HookInjectedItems` / `SystemItems`) は **読み出し専用 (deserialize-only)** として残置。 既存セッションログを開けることは保証する
- `collect_state` は新旧両方をフラットに `state.history` に展開
- 1 entry = 1 item になることで commit 経路が `LogEntry::<Singular> { item }` に直接 1:1 写像する
- wire 側の `Event::SystemItem` (per-item) と log 上の `LogEntry::SystemItem` (per-item) が完全対称になり、 IPC server の fan-out ロジックが消える

ハッシュチェーン長は entry 数に比例して長くなるが、 hash chain 自体は別チケットで廃止予定なので長さの悪化は受け入れる。

## 要件

- `Store` trait の主要メソッド (`append` / `read_all` / `read_head_hash` / `create_session` 等) を sync API に統一する。 async 版を残す必要があるかは内部判断 (例: 起動時に大きい log を読む `read_all` だけ async を残す案もあり、 必要に応じて選択)
- `SessionLogWriter` (Pod 側ラッパ) を sync API に統一し、 hash chain 計算 + session_head 更新 + sink publish を 1 つの sync 関数内で行う
- `session_head` を sync mutex に変更
- `LogCommand` / `LogDrainHandle::run_log_drain` / `log_cmd_tx` / `log_cmd_rx` / Flush バリアを削除
- `PodInterceptor::on_prompt_submit` / `pending_history_appends` は `Arc<SessionLogWriter>` を直接呼んで per-`SystemItem` に commit。 完了後に `Item::system_message` を worker に返す
- worker の `on_history_append` callback が直接 `writer.append_entry(...)` を呼ぶ (sync)
- `LogEntry` に `AssistantItem` / `ToolResult` / `SystemItem` の単数 variant を追加し、 新規書き込みはこれを使う
- 旧 plural variant は read 専用として残し、 deserialize alias または独立 variant のどちらでも実装判断で良い。 `collect_state` で同じ形に reduce されること
- IPC server の `LogEntry::SystemItem` 受信は 1 件の `Event::SystemItem` をそのまま送る単純な対応に
- `cargo test --workspace` が pass する

## 完了条件

- `LogCommand` / drain task / Flush バリアがコードから消えている
- worker の `on_history_append` callback が直接 sync writer を呼んでいる
- `PodInterceptor` が writer を直接呼んで SystemItem を commit している (mpsc 経由ではない)
- `Store` および `SessionLogWriter` の主要 commit API が sync 関数
- `session_head` の mutex 型が `parking_lot::Mutex` または `std::sync::Mutex`
- 新規セッションログに `assistant_item` / `tool_result` / `system_item` (snake_case wire tag) が単数 entry として書かれている
- 旧 `assistant_items` / `tool_results` / `hook_injected_items` / `system_items` を含むセッションログが読めて view 復元できる
- IPC server で `LogEntry::SystemItem` を受けたら 1 件の `Event::SystemItem` を fan-out 無しで送るシンプルなマッピングに戻っている

## 範囲外

- ハッシュチェーン自体の廃止 (entry hash, `prev_hash`, `HashedEntry`, `session_head` mutex の最終撤去) は `tickets/persistence-semantics.md` の責務領域。 本チケットはその直前段階として、 mutex を sync 化するところまで
- Session / Segment 階層への rename (`tickets/persistence-semantics.md`)
- 旧 plural entry の disk migration (ファイル書き換え)。 deserialize alias で読めるところまで
- `Event::SystemItem` payload の変更 (引き続き `serde_json::Value` で `SystemItem` の JSON 形)

## 関連

- 前提 `tickets/system-item-unify.md` (本チケットで完成した SystemItem 経路を、 本チケットで単数化 + direct writer 化する後続)
- 関連 `tickets/pod-state-from-session-log.md` (per-item commit の drain task 経路を確立した前段。 本チケットでその drain task 自体を撤去する)
- 後続 `tickets/persistence-semantics.md` の「Entry hash の廃止」 セクション。 本チケットで sync 化したことが entry hash 廃止の足場になる
