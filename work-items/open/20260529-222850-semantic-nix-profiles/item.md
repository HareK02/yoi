---
id: 20260529-222850-semantic-nix-profiles
slug: semantic-nix-profiles
title: Make Nix profiles semantic and manifestize them in the resolver
status: open
kind: task
priority: P2
labels: [manifest, profiles, nix, architecture]
created_at: 2026-05-29T22:28:50Z
updated_at: 2026-05-29T22:28:50Z
assignee: null
legacy_ticket: null
---

## Background

The closed manifest profiles work item (`work-items/closed/20260527-000022-manifest-profiles`) introduced profile discovery/selection, built-in profile files, and Nix evaluation as the profile artifact path. Its original direction described profiles as role/model/policy selections that resolve into a precise runtime manifest. The current built-in Nix shape has drifted from that intent: it effectively asks authors to write a Manifest-shaped object in Nix.

Current examples include:

- `resources/nix/profile-lib.nix` exposes `mkProfile` and a `mkManifest` helper, where `mkManifest` is currently an identity function.
- `resources/nix/profiles/default.nix` uses `insomnia.mkProfile { manifest = insomnia.mkManifest { ... }; }`.
- The built-in profile currently contains low-level or instance-specific Manifest fields such as `pod.name = "insomnia"`.
- Model policy fields such as high reasoning effort and compaction thresholds are represented as low-level manifest settings rather than being derived from a profile's model/context policy.
- Profile evaluation currently shells out to `nix eval` for Nix artifacts, so the boundary between profile language support and a runtime dependency on the `nix` command is unclear.

The issue is not that profile artifacts are eventually serialized as manifests. The issue is that the authoring API currently exposes the output shape as if the profile itself were a manifest. Profiles should be semantic, and the resolver should perform the manifestization step.

## Goal

Redesign the Nix profile authoring surface so profiles express role/model/tool/context policy at the appropriate level of abstraction, and the manifest resolver turns that semantic profile into the concrete runtime manifest/config used to start a Pod.

This should restore the intended boundary:

- Profile: selectable role/policy/model/context/tool behavior.
- Resolver/manifestization: derives concrete defaults and token thresholds from model metadata and profile policy.
- Runtime manifest/config: low-level concrete values consumed by Pod startup.

## Desired direction

- Remove `mkManifest` from the public/built-in authoring style. A profile should not look like `mkProfile { manifest = mkManifest { ... }; }`.
- Built-in profiles should be written as semantic profiles, not Manifest-shaped blobs.
- Instance-specific values such as `pod.name` must not live in built-in profiles. Pod names come from CLI/TUI/SpawnPod creation, not profile identity.
- Model behavior such as reasoning effort belongs to the profile's model/quality policy and should be manifestized into provider-specific request config by the resolver.
- Compaction and context budgeting should be derived from the selected model's context window and profile policy, e.g. ratios/multipliers/strategy names, rather than hard-coded token thresholds copied into the profile.
- Nix evaluation should be treated as a profile authoring/evaluation mechanism, not as a requirement that the runtime default path always depends on an external `nix` command.

## Acceptance criteria

- Audit the original manifest profiles work item and current implementation to identify where the implementation became Manifest-shaped instead of semantic-profile-shaped.
- Replace the built-in Nix profile shape with a semantic profile API. The exact field names may be redesigned, but they must clearly distinguish profile policy from runtime manifest output.
- Remove `pod.name` and any other instance-specific Pod identity/configuration from built-in profiles.
- Remove `mkManifest` from built-in examples and docs. If a compatibility helper remains internally, it must not be the recommended authoring API and must be justified.
- Implement or adjust the resolver so semantic profile data is manifestized into the concrete runtime manifest/config.
- Ensure model-derived settings are resolved in one place. In particular, reasoning effort and context/compaction policy should be interpreted as profile/model policy and converted into concrete request/compaction settings by the resolver.
- Add or update model catalog/context-window plumbing as needed so compaction thresholds can be derived from the selected model rather than duplicated as raw profile constants.
- Decide and document the Nix evaluator boundary:
  - built-in/default profiles must not require the external `nix` command at normal runtime unless that dependency is deliberately accepted and documented;
  - user/project Nix profiles may use `nix eval` if no lightweight in-process evaluator is chosen, but diagnostics must clearly state that Nix is required for Nix-authored profiles;
  - do not silently make all profile selection depend on a missing or slow `nix` binary.
- Investigate whether a lightweight embedded evaluator is practical enough for the supported Nix subset. If it is not, document why and keep the external-command dependency isolated to Nix-authored profile evaluation.
- Preserve previous profile selection semantics where still relevant: built-in/user/project profile discovery, default profile selection, source-qualified selectors, ambiguity errors, and manifest cascade removal.
- Update docs/tests so the examples present profiles as semantic policy, not manifests written in Nix syntax.
- Add tests covering:
  - built-in profile manifestization without `pod.name`;
  - semantic compaction policy derived from model context window;
  - reasoning/model policy manifestization;
  - missing `nix` command behavior for user/project Nix profiles, if external evaluation remains;
  - no regression in profile discovery/default/source-qualified selection.
- Run focused validation for the manifest crate and any affected pod/tui/client paths, plus `./tickets.sh doctor` and `git diff --check`.

## Non-goals

- Do not reintroduce manifest cascade or manifest overlay selection as a replacement for profiles.
- Do not turn Nix profiles into a general low-level manifest DSL.
- Do not add profile alias registry complexity back unless a separate ticket re-justifies it.
- Do not implement encrypted secret storage here; keep it in the existing encrypted secrets work item.
- Do not require solving every possible provider-specific model option in this ticket; implement the minimum necessary semantic-to-runtime mapping and leave explicit follow-ups for broader model policy coverage.
