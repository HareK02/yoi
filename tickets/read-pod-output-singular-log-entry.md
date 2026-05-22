# ReadPodOutput: singular LogEntry 形式への追従

## 背景

`ReadPodOutput` は spawned Pod の socket に接続して `Event::Snapshot` を読み、差分 cursor 以降の assistant text を抽出する。現在の抽出処理は `SegmentStart.history` と legacy plural の `LogEntry::AssistantItems` だけを見ている。

一方、現在の session log は新規書き込みで singular の `LogEntry::AssistantItem` を使う。実際に attach では assistant 出力が見えているのに、spawner 側の `ReadPodOutput` が `no new assistant text` を返す事象が確認された。これは hash cursor の問題ではなく、`extract_assistant_text` が新しい LogEntry 形式を読めていないことが主因と見られる。

## 要件

- `crates/pod/src/spawn/comm_tools.rs` の `extract_assistant_text` を現在の `LogEntry` 形式に追従させる。
  - `LogEntry::AssistantItem { item, .. }` を assistant text 抽出対象に含める。
  - legacy `AssistantItems` と `SegmentStart.history` の扱いは維持する。
  - 必要なら `SystemItem` / tool result / reasoning item は対象外でよいが、assistant message item の取りこぼしはなくす。
- `ReadPodOutput` の cursor は item index ベースのままでよいが、抽出対象を見落として cursor だけ進む退行を test で防ぐ。
- attach/TUI に見える assistant output と `ReadPodOutput` の結果が乖離しないようにする。

## 完了条件

- singular `LogEntry::AssistantItem` を含む snapshot に対して、`ReadPodOutput` が assistant text を返す unit/integration test を追加する。
- legacy `LogEntry::AssistantItems` の既存 test が引き続き通る。
- cursor の2回目 read は引き続き `no new assistant text` になる。
- `cargo fmt --check`
- `cargo check --workspace`
- `cargo test -p pod`

## 範囲外

- cursor 永続化
- ReadPodOutput の protocol/API 変更
- 過去 Pod 探索 / restore tool の実装
- assistant text 以外の structured event 表示
