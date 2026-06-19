Ticket `00001KVFD3YSV` is complete.

Completed implementation:
- Added read-only Plugin inspection CLI commands:
  - `yoi plugin list`
  - `yoi plugin show <ref>`
  - JSON output support.
- Added typed Plugin inspection report used by both JSON and human output.
- Inspection reports package path/location, schema/API version, source/ref/digest/version, requested permissions, grants/denials, diagnostics, Tool/static eligibility, and future host API eligibility structure.
- Status vocabulary is limited to `active`, `disabled`, `missing`, `rejected`, `partial`.
- Implemented static Tool definition inspection for invalid/duplicate Tool names and invalid `input_schema`.
- Distinguished truly absent configured package refs (`missing`) from present-but-invalid packages (`rejected`), including `Missing` diagnostics for missing root `plugin.toml` or missing referenced runtime/package entries.
- Preserved read-only/no-execution behavior: inspection does not execute Plugin WASM or Tool code.
- Kept diagnostics bounded and structured.

Reviewed / merged:
- Implementation commits:
  - `462de32a` (`plugin: add cli inspection`)
  - `b5f10ab7` (`plugin: align inspection statuses`)
  - `dfa966db` (`plugin: report inspection package metadata`)
  - `982a1b75` (`plugin: validate inspected tool schemas`)
  - `a5f3b0b5` (`plugin: reject configured invalid packages`)
  - `0142ef1d` (`plugin: distinguish present invalid packages`)
- Multiple review rounds requested and verified fixes for status vocabulary, package metadata fields, Tool schema/name validation, configured invalid package status, and present-but-invalid `Missing` diagnostics.
- Final review `yoi-reviewer-00001KVFD3YSV-r6` approved with no blockers.
- Orchestrator merge commit: `71ca05c8` (`merge: plugin cli inspection`)

Validation in Orchestrator worktree:
- `cargo fmt --check` — passed
- `cargo check -p yoi -p pod -p manifest` — passed
- `cargo test -p yoi plugin -- --nocapture` — passed; 11 passed, 0 failed
- `cargo test -p pod static_inspection -- --nocapture` — passed; 4 passed, 0 failed
- `cargo test -p pod plugin -- --nocapture` — passed; 31 passed, 0 failed
- `cargo test -p manifest plugin -- --nocapture` — passed; 17 passed, 0 failed
- `git diff --check` — passed
- `nix build .#yoi --no-link` — passed

Cleanup:
- Stopped Coder Pod `yoi-coder-00001KVFD3YSV`.
- Stopped Reviewer Pod `yoi-reviewer-00001KVFD3YSV-r6`.
- Removed child worktree `/home/hare/Projects/yoi/.worktree/00001KVFD3YSV-plugin-cli-inspection`.
- Deleted merged branch `impl/00001KVFD3YSV-plugin-cli-inspection`.

Root/original workspace was not read/written/merged/validated for this Ticket, per Panel Queue instruction. The completed work is integrated on the Orchestrator branch.