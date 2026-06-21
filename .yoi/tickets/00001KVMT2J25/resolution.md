In-flight LLM response 中の reconnect / late attach snapshot に unfinished blocks を含める protocol/pod/TUI 実装を統合した。

主な成果:
- `Event::Snapshot` に typed `InFlightSnapshot` / `InFlightBlock` を追加。
- Pod 側に assistant text / thinking / tool-call args の in-flight accumulator を追加。
- Streaming callbacks が accumulator 更新と live delta broadcast を同じ stream path で行うようにした。
- Connect-time snapshot が in-flight stream state を含むようにした。
- Session-log mirror snapshot と in-flight snapshot、および finalized `AssistantItem` publish/clear の critical section を揃え、mirror-only assistant commit が snapshot/live boundary で消える race を防止した。
- Finalized assistant item が committed snapshot に含まれる場合は matching in-flight state を clear して duplicate を防ぐ。
- TUI snapshot restore が unfinished text/thinking/tool-call args blocks を seed し、後続 live deltas が同じ logical block に continuation されるようにした。
- Serialization/default compatibility、snapshot/live no-gap/no-duplicate、TUI continuation の focused regression tests を追加。

統合・検証:
- Merge commit: `b21638f5 merge: inflight reconnect snapshot`
- Implementation commits: `74aca6f6`, `061136d7`
- Reviewer final verdict: approve
- Validation passed: `cargo fmt --all --check`, `git diff --check HEAD^1..HEAD`, `cargo test -p protocol`, `cargo test -p pod --lib in_flight`, `cargo test -p pod session_log_and_in_flight_snapshot_prevents_mirror_only_assistant_gap`, `cargo test -p pod committed_assistant_snapshot_does_not_duplicate_in_flight_block`, `cargo test -p tui snapshot_in_flight_blocks_continue_with_live_deltas`, `cargo test -p tui`, `cargo check -p protocol -p pod -p tui`, and `cargo run -p yoi -- ticket doctor`。

既知の無関係事項:
- Full `cargo test -p pod` は既存の prompt-resource assertion 2 件で失敗することが reviewer により確認済み。この Ticket の差分とは無関係。