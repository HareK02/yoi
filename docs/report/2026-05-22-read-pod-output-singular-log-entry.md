# ReadPodOutput が新しい LogEntry 形式を読めず no new assistant text になる

## 観測

spawned Pod のレビュー出力について、operator が attach すると assistant 出力が見える一方、spawner 側の `ReadPodOutput` は `pod ... running; no new assistant text` を返した。

## 原因候補

`crates/pod/src/spawn/comm_tools.rs` の `extract_assistant_text` は、`Event::Snapshot` 内の `LogEntry` から assistant text を抽出する際に以下だけを対象にしている。

- `LogEntry::SegmentStart { history, .. }`
- legacy plural の `LogEntry::AssistantItems { items, .. }`

現在の session log は新規書き込みで singular の `LogEntry::AssistantItem { item, .. }` を使うため、新しい assistant 出力が snapshot に存在しても抽出対象にならない。その結果、`ReadPodOutput` は cursor を進めつつ text は空と判断し、以後の read でも見えなくなる。

これは entry hash の問題ではなく、session-store の新しい singular LogEntry 形式への tool 側追従漏れと見られる。

## 対応

`tickets/read-pod-output-singular-log-entry.md` を追加した。`extract_assistant_text` に `LogEntry::AssistantItem` 対応を追加し、singular entry の snapshot から text を返す test を追加する。
