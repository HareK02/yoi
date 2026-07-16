Extract staging の entry-level source anchors / evidence references を実装・レビュー・merge・検証した。

実装内容:
- `crates/memory/src/schema/common.rs` に bounded source/evidence metadata 型を追加。
  - `EvidenceKind(pub String)`。
  - known constants: `message`, `tool_call`, `tool_result`, `file_ref`, `ticket_ref`, `objective_ref`。
  - `SourceEvidenceRef` with `session_id`, `segment_id`, `entry_range`, `evidence_id`, `evidence_kind`, `label`, `summary`。
- `DecisionEntry`, `DiscussionEntry`, `AttemptEntry`, `RequestEntry` に optional `source_refs: Vec<SourceEvidenceRef>` を追加。
- `source_refs` は default empty / skip serializing empty とし、`#[schemars(skip)]` で LLM extract schema には露出しないようにした。
- 既存 `StagingRecord.source` は record-level source range として維持。
- old staging JSON without `source_refs` の互換性を維持。
- legacy record-level `source.session_id` compatibility を追加。
- new staging JSON with entry-level refs の roundtrip を追加。
- consolidation input は pretty JSON として entry-level refs を lossless に保持するようにした。
- session-explore / resolver tools / extract launch / staging resolution/disposition はこの Ticket では実装していない。

Review:
- Reviewer approved with no blockers。
- Evidence included bounded anchor field definitions, all four entry types, old/new JSON tests, legacy source compatibility, and consolidation preservation。
- Non-blocking note: `label` / `summary` の length/raw-content discipline は type enforcement ではなく semantic/host-policy に残る。

Merge / validation:
- Merge commit: `9a1a75de merge: staging source anchors`。
- Final validation passed:
  - `cargo fmt --check`
  - `git diff --check`
  - `cargo test -p memory`
  - `cargo check -p yoi`
  - `yoi ticket doctor`
  - `nix build .#yoi --no-link`
- Validation log: `/run/user/1000/yoi/yoi-orchestrator/bash-output/staging-source-anchors-final-validation-1784231070.txt`

Cleanup:
- Implementation worktree/branch cleanup will be performed after close commit。
- Per user instruction, `StopPod` is not used。