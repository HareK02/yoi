# ReadPodOutput が LogEntry schema の分岐に取り残され no new assistant text になる

## 観測

spawned Pod のレビュー出力について、operator が attach すると assistant 出力が見える一方、spawner 側の `ReadPodOutput` は `pod ... running; no new assistant text` を返した。

## 原因

`crates/pod/src/spawn/comm_tools.rs` の `extract_assistant_text` が、`Event::Snapshot` 内の `LogEntry` を独自に解釈していた。session log の標準形は `LogEntry::AssistantItem { item, .. }` だが、`ReadPodOutput` 側の抽出対象が古い複数 item 形式に寄ったままになっていたため、新しい assistant 出力が snapshot に存在しても取りこぼした。

これは entry hash の問題ではなく、`LogEntry` に対する派生操作が各所で独自実装され、後方互換 variant が残ったことで標準形への追従漏れを型で検出できなかった問題。

## 対応

`LogEntry` から古い複数 item 形式の後方互換 variant を削除し、`ReadPodOutput` / TUI / picker / tests を現在の singular entry だけに揃える。これにより schema 変更時に独自 match の取り残しが compile error として表面化する。
