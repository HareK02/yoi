# Investigation and plan: Ticket config role profile mapping

## Conclusion

Implement `.yoi/ticket.config.toml` as Ticket orchestration configuration with fixed Ticket role slots. Do not build a generic Role registry.

The initial implementation should parse/validate the config, provide defaults, expose role-to-profile and prompt/workflow references, and wire the configured backend root into the existing Ticket built-in feature adapter. Pod spawning, TUI actions, and workflow state should remain follow-up work.

## Design position

Use fixed Ticket roles:

- `intake`
- `orchestrator`
- `coder`
- `reviewer`
- `investigator`

These are not arbitrary user-defined roles. They are the roles required by the Ticket feature/workflows.

Keep the boundary:

- Profile: Pod runtime recipe.
- Ticket role config: binds a fixed Ticket role to a Profile selector and optional prompt/workflow refs.
- System instruction: role's durable behavior/boundary.
- Launch prompt: first committed task/user message for a concrete Ticket/action.
- Workflow: procedural flow, later possibly stateful.

## Current code map

### Ticket backend/tools

- `crates/ticket/src/lib.rs`
  - Owns Ticket domain/backend and `LocalTicketBackend`.
  - Current backend root is supplied by callers.
- `crates/ticket/src/tool.rs`
  - Owns Ticket tool input/output and `llm_worker::Tool` implementations.
- `crates/pod/src/feature/builtin/ticket.rs`
  - Thin built-in feature adapter.
  - Currently resolves `<workspace>/work-items` directly.
  - This is the first integration point for `.yoi/ticket.config.toml` backend root.

### Feature/host authority

- `crates/pod/src/feature.rs`
  - Defines `HostAuthority::TicketBackend { root }`.
  - Feature descriptor/install path already supports requested host authority and tool contribution wiring.

### Profile selection

- `crates/manifest/src/profile.rs`
  - Owns Profile registry/selector resolution.
  - `SpawnPod.profile` already accepts selectors such as `inherit`, default/source-qualified/unambiguous registry names.
- `crates/pod/src/spawn/tool.rs`
  - Implements `SpawnPod` tool input with optional profile selector and `inherit` semantics.
  - This should remain the actual spawning boundary; config should not duplicate full profile resolution behavior.

### Workflow resources

- `.yoi/workflow/*.md`
  - Current workflow files are project-authored resources.
  - `ticket-intake-workflow.md`, `ticket-orchestrator-routing.md`, `ticket-preflight-workflow.md`, and `multi-agent-workflow.md` are the relevant workflow refs.
- `crates/workflow/src/workflow.rs`
  - Parses workflow frontmatter/body records.
  - No stateful workflow runner exists yet.

### Prompt resources / system instruction

- `crates/pod/src/prompt/loader.rs`
  - Resolves instruction-file references like `$yoi/...` and `$user/...` for current startup/instruction use.
- Prompt catalog/resources are currently separate from workflow state.
- There is no implemented role-specific launch prompt engine yet.

## Important constraint

Do not make `ticket` depend on `pod`.

Possible dependency choices:

1. Put config parsing in `ticket` crate with raw profile/prompt/workflow string refs.
   - Pros: Ticket config is close to Ticket backend concept.
   - Cons: `ticket` learns about profile/prompt/workflow reference strings, but not their runtime resolution.

2. Put config parsing in `pod`.
   - Pros: avoids exposing prompt/profile concepts from `ticket`.
   - Cons: Ticket config becomes less reusable by future CLI/TUI code unless those crates also depend on `pod`.

Recommended MVP:

- Add config domain/parser to `crates/ticket`, using lightweight string wrapper types such as `ProfileSelectorRef`, `PromptRef`, and `WorkflowRef` without depending on `manifest` or `pod`.
- `pod` consumes this config and performs runtime interpretation where needed.

This preserves:

```text
ticket -> llm-worker / serde / toml only
pod -> ticket
```

and avoids:

```text
ticket -> pod
```

## Proposed schema

```toml
[backend]
kind = "local"
root = "work-items"

[roles.intake]
profile = "project:intake"
system_instruction = "$workspace/ticket/intake/system"
launch_prompt = "$workspace/ticket/intake/launch"
workflow = "ticket-intake-workflow"

[roles.orchestrator]
profile = "project:orchestrator"
system_instruction = "$workspace/ticket/orchestrator/system"
launch_prompt = "$workspace/ticket/orchestrator/launch"
workflow = "ticket-orchestrator-routing"

[roles.coder]
profile = "inherit"
system_instruction = "$workspace/ticket/coder/system"
launch_prompt = "$workspace/ticket/coder/launch"
workflow = "multi-agent-workflow"

[roles.reviewer]
profile = "project:reviewer"
system_instruction = "$workspace/ticket/reviewer/system"
launch_prompt = "$workspace/ticket/reviewer/launch"
workflow = "multi-agent-workflow"

[roles.investigator]
profile = "inherit"
system_instruction = "$workspace/ticket/investigator/system"
launch_prompt = "$workspace/ticket/investigator/launch"
workflow = "ticket-orchestrator-routing"
```

The specific prompt ref syntax should be accepted as opaque strings in this ticket. Runtime prompt resolution belongs to the later role launcher.

## Defaults

When `.yoi/ticket.config.toml` is missing:

- backend kind: `local`
- backend root: `work-items`
- role profiles: `inherit`
- workflow defaults:
  - intake: `ticket-intake-workflow`
  - orchestrator: `ticket-orchestrator-routing`
  - coder: `multi-agent-workflow`
  - reviewer: `multi-agent-workflow`
  - investigator: `ticket-orchestrator-routing`
- system instruction / launch prompt: none

When a role section exists but omits optional prompt/workflow refs:

- keep configured profile;
- fill workflow default for the fixed role;
- leave prompt refs as none.

## Implementation plan

### Phase 1: Config model/parser in `ticket`

Add a module such as `crates/ticket/src/config.rs`.

Types:

```rust
pub struct TicketConfig {
    pub backend: TicketBackendConfig,
    pub roles: TicketRoleProfiles,
}

pub struct TicketBackendConfig {
    pub kind: TicketBackendKind,
    pub root: PathBuf,
}

pub enum TicketBackendKind {
    Local,
}

pub enum TicketRole {
    Intake,
    Orchestrator,
    Coder,
    Reviewer,
    Investigator,
}

pub struct TicketRoleProfile {
    pub profile: ProfileSelectorRef,
    pub system_instruction: Option<PromptRef>,
    pub launch_prompt: Option<PromptRef>,
    pub workflow: WorkflowRef,
}
```

Use string wrapper types for selectors/refs to avoid depending on `manifest`/`pod`.

Parsing behavior:

- `TicketConfig::load_workspace(workspace_root: &Path)` reads `.yoi/ticket.config.toml` if present.
- Missing file returns defaults.
- Relative backend root resolves against workspace root.
- Unknown roles are errors.
- Unknown top-level fields should be diagnostics/errors rather than silently ignored.
- Backend kind supports only `local` for now.

### Phase 2: Wire backend root into Pod Ticket feature adapter

Update `crates/pod/src/feature/builtin/ticket.rs`:

- Load `TicketConfig` from workspace root.
- Use `config.backend.root` instead of hard-coded `workspace/work-items`.
- Preserve current fail-closed behavior if root is missing/unusable.
- Keep `HostAuthority::TicketBackend { root }` consistent with the validated/canonical root where practical.

This directly improves existing Ticket tools without introducing role spawning yet.

### Phase 3: Tests

Ticket crate tests:

- missing config -> defaults;
- full config parses;
- partial role config uses role workflow defaults;
- unknown role rejects;
- unsupported backend kind rejects;
- relative backend root resolves against workspace;
- malformed profile/ref diagnostics are bounded.

Pod tests:

- Ticket built-in feature uses configured backend root;
- missing/unusable configured backend root does not register tools;
- default missing config still uses `<workspace>/work-items`.

### Phase 4: Documentation/example

Add one of:

- a short `.yoi/ticket.config.example.toml`, or
- a documented snippet under the ticket implementation report / docs if adding tracked config now is too early.

For this repository, adding actual `.yoi/ticket.config.toml` should be considered carefully. If added, defaults should likely use `inherit` profiles until dedicated profiles exist.

## Deferred follow-ups

### `ticket-role-pod-launcher`

- Take TicketRole + Ticket context + role config.
- Build `SpawnPod` requests.
- Resolve profile selector using existing Profile registry.
- Resolve role system instruction separately from initial launch prompt.
- Commit launch prompt as the first user/task message, not hidden context.
- Include workflow ref in launch/task context.

### `tui-ticket-role-actions`

- Add TUI actions for fixed Ticket roles:
  - Intake/refine Ticket;
  - Route Ticket;
  - Investigate;
  - Implement;
  - Review.
- Use the launcher rather than building SpawnPod requests inside UI code.

### Stateful workflow engine

- Persist workflow phase/state.
- Gate allowed tools by phase.
- Inject phase prompts only by committing them to history first.
- Keep SystemInstruction role-stable and task/phase prompts dynamic.

## Validation for implementation

Required:

- `cargo test -p ticket`
- `cargo test -p pod ticket --lib`
- `cargo test -p pod feature --lib`
- `cargo check --workspace --all-targets`
- `cargo fmt --check`
- `git diff --check`
- `./tickets.sh doctor`

Optional if feasible:

- `nix build .#yoi --no-link`
