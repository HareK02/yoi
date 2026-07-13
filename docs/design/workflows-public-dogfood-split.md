# Public builtin workflows and Yoi dogfood workflows

Yoi should split workflows into public builtin procedures and project-local dogfood procedures. The builtin set should teach the general Ticket/role lifecycle; this repository's Git worktree, branch, cargo, nix, merge, cleanup, and report conventions should stay project-local unless they are explicitly selected as an optional extension.

This document records the design/audit decision for Ticket `00001KTRKZ14C`. It does not implement the builtin workflow loader.

## Decision

- Add a typed builtin workflow source, conceptually `WorkflowSource::Builtin`, with clear provenance in any list/show/launch surface.
- Store public builtin workflow resources under `resources/workflows/<slug>.md`, not under `resources/prompts`.
  - Workflows are user-visible procedural artifacts selected by slug and source. They need source/provenance/listing semantics distinct from prompt fragments.
  - They are still embedded runtime resources, but prompt-fragment override rules should not silently become workflow authority.
- Source priority for workflow lookup should be:
  1. explicit path/file selector, if the CLI/config surface allows one;
  2. workspace `.yoi/workflow/<slug>.md`;
  3. user workflow directory, if/when one is added;
  4. builtin `resources/workflows/<slug>.md`;
  5. external skill/plugin workflow sources, only when explicitly enabled and named.
- Workspace workflows override builtin workflows by slug. Overrides must show provenance so a role launch can say whether it used workspace, user, builtin, or plugin workflow text.
- Keep role Profile policy separate from workflow prose. Profiles select role behavior/tool policy; workflows provide procedural flow; launch prompts provide concrete Ticket/action context.
- Public resident core should remain small: intake requirements sync and orchestrator routing. Multi-agent implementation/review is builtin-available but not necessarily resident in every profile. Git worktree mechanics are optional/project-local by default.

## Audit of current workspace workflows

| Current slug | Public builtin candidate | Dogfood/project-local material | Decision |
| --- | --- | --- | --- |
| `ticket-intake-workflow` | Yes. The core requirement-sync flow, existing-Ticket refinement, readiness, duplicate avoidance, and Ticket materialization are product-generic. | Mentions current local storage and Yoi-specific Ticket command examples. | Move cleaned generic version to builtin core. Keep storage-command specifics either bounded and product-generic or in project-local notes. |
| `ticket-orchestrator-routing` | Yes. The routing classifications, queued acceptance contract, relation/orchestration-plan checks, and planning-return rules are product-generic. | Contains Yoi dogfood examples, worktree/multi-agent handoff specifics, and merge-completion guidance tied to this repo's operating policy. | Move generic routing gate to builtin core. Split dogfood merge/worktree details into workspace override or separate dogfood workflow. |
| `ticket-preflight-workflow` | Compatibility candidate only. It now describes planning/requirements sync and legacy canonical id compatibility rather than a standalone lane. | Mostly project-history wording and deprecated-preflight vocabulary. | Do not make resident core. Keep as builtin compatibility alias only if existing configs still reference it; otherwise replace references with intake/planning sync. |
| `multi-agent-workflow` | Partly. The sibling coder/reviewer loop, intent packet, branch-local review, and merge-ready dossier are useful product concepts. | Heavy Yoi dogfood specifics: Git worktrees, commits, cargo/nix validation, branch cleanup, docs/report conventions, and parent/child policy. | Split into builtin `multi-agent-workflow` for role loop + dossier, and workspace dogfood extension for Git worktree/cargo/nix mechanics. |
| `worktree-workflow` | Not resident public core. | Almost entirely this repository's Git worktree mechanics and `.yoi` path exclusions. | Keep workspace-local dogfood workflow. Optionally create a non-resident builtin `git-worktree-isolation` later, disabled unless explicitly selected. |

## Slug and `.yoi/ticket.config.toml` migration decision

Use explicit dogfood slugs for workflows whose semantics differ from the public builtin workflow. Same-slug workspace overrides are allowed for local wording/policy tweaks of the same public contract, but they should not be used to hide Yoi repository Git/worktree/cargo/nix/merge semantics behind a public slug. This avoids accidental shadowing when a user expects `multi-agent-workflow` to mean the generic builtin role loop.

Planned selector mapping for this repository after builtin workflow loading exists:

| Role/config surface | Workflow selector | Source intent | Rationale |
| --- | --- | --- | --- |
| `[roles.intake].workflow` | `ticket-intake-workflow` | builtin core by default; workspace same-slug override only if this repo needs a true local refinement of the same intake contract | Intake is mostly product-generic; dogfood Git/worktree policy does not belong here. |
| `[roles.orchestrator].workflow` | `ticket-orchestrator-routing` | builtin core by default; dogfood merge/worktree instructions come from explicit launch context or a separate dogfood workflow, not a hidden same-slug override | Routing semantics should stay public/generic; repo-specific implementation mechanics should be opt-in and visible. |
| `[roles.coder].workflow` | `yoi-dogfood-multi-agent-workflow` | workspace-local dogfood workflow | Coder work in this repository is tied to the repo worktree/branch/validation contract and must not shadow generic builtin `multi-agent-workflow`. |
| `[roles.reviewer].workflow` | `yoi-dogfood-multi-agent-workflow` | workspace-local dogfood workflow | Reviewer evidence in this repository includes branch/worktree/cargo/nix/Ticket-doctor conventions; use an explicit dogfood slug. |
| compatibility references | `ticket-preflight-workflow` | builtin compatibility alias or workspace compatibility file, non-resident | Do not keep it as a role default. It should point to planning/requirements sync language only. |
| dogfood worktree helper | `yoi-dogfood-worktree-workflow` | workspace-local helper, not a role default by itself | Referenced from `yoi-dogfood-multi-agent-workflow`; keeps Git worktree mechanics out of generic builtin workflows. |

`multi-agent-workflow` should be reserved for the generic builtin sibling coder/reviewer loop. This repository should not keep a same-slug workspace override with dogfood semantics once the builtin exists. If a temporary same-slug override is needed during migration, it must be short-lived and the Ticket/config comment must say it shadows the builtin intentionally.

## Builtin workflow set

### Resident builtin core

These should be advertised by default when Ticket support is enabled:

- `ticket-intake-workflow`
  - generic Ticket requirements sync/materialization;
  - existing Ticket refinement with explicit user agreement;
  - readiness and duplicate handling;
  - no Yoi repository cargo/nix/worktree policy.
- `ticket-orchestrator-routing`
  - explicit routing gate;
  - queued acceptance before implementation side effects;
  - relation/orchestration-plan/context checks;
  - implementation-ready IntentPacket shape;
  - no repository-specific merge authority or Git cleanup policy.

### Builtin available but optional

These can be embedded and selectable, but should not be injected as resident core unless the active Profile/config opts in:

- `multi-agent-workflow`
  - generic sibling coder/reviewer loop;
  - intent packet, reviewer evidence, blocker/fix-loop, merge-ready dossier;
  - no mandatory Git worktree, cargo, nix, or `.yoi` cleanup rules.
- `ticket-preflight-workflow`
  - transitional alias for old configurations that still name preflight;
  - should route to planning/requirements sync language and avoid reintroducing preflight as a lifecycle state.

### Workspace-local dogfood workflows

These remain in this repository under `.yoi/workflow/` as overrides or dogfood-only slugs:

- `worktree-workflow`
  - mechanical Yoi project worktree setup;
  - tracked `.yoi` project records visible;
  - `.yoi/memory`, local/runtime/log/lock/secret-like paths excluded;
  - branch/worktree cleanup policy.
- A dogfood extension for multi-agent/merge-completion, if the generic builtin `multi-agent-workflow` is cleaned:
  - concrete `cargo fmt`, `cargo test`, `target/debug/yoi ticket doctor`, validation policy;
  - merge authority boundaries for this repository;
  - docs/report conventions.

## Stale vocabulary cleanup

Builtin workflows and active workspace workflows should remove or replace the following old wording:

- `Action required` / `Attention required` as storage or state concepts. Use explicit Ticket state, typed relation metadata, orchestration-plan waiting notes, or Ticket thread comments instead.
- `preflight` as a lifecycle lane/state. Use planning/requirements sync or compatibility alias language.
- `open` / `pending` / `closed bucket` authority. Current authority is `state` plus optional `resolution.md` for closed Tickets.
- Long-lived umbrella/progress-container Ticket terminology. Use concrete implementation Tickets and optional Objective context.

## Follow-up implementation boundaries

1. **Builtin workflow resource loader**
   - Add embedded `resources/workflows/*.md` resources.
   - Add `WorkflowSource::Builtin` / equivalent typed provenance.
   - Update workflow discovery/list/show/launch so workspace overrides builtin by slug and provenance is visible.
   - Include tests for source priority and override behavior.

2. **Public workflow text cleanup**
   - Create cleaned builtin versions of `ticket-intake-workflow` and `ticket-orchestrator-routing`.
   - Split generic `multi-agent-workflow` from Yoi dogfood Git/worktree/cargo/nix mechanics.
   - Keep `ticket-preflight-workflow` as compatibility-only or remove references if no longer needed.

3. **Dogfood workflow retention**
   - Keep or rename workspace-local dogfood workflows for this repository's implementation mechanics.
   - Ensure `.yoi/ticket.config.toml` role workflow selectors intentionally point to builtin generic workflows or workspace dogfood overrides, with no accidental slug shadowing.

4. **Vocabulary migration**
   - Sweep active workflow text, prompts, docs, and Ticket UI wording for `Action required`, `Attention required`, and old preflight lane language.
   - Replace with typed state/relation/orchestration-plan terminology.

5. **Panel/role launch provenance display**
   - When a role launches with a workflow, include the workflow source/provenance in diagnostic/launch context so builtin vs workspace override is debuggable.

## Non-goals for this Ticket

- Implementing the builtin workflow loader.
- Moving current workflow files into runtime resources in this branch.
- Changing Ticket lifecycle states.
- Changing role Profile policy.
- Changing scheduler/queue behavior.
