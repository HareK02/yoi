# Semantic Nix profiles implementation plan

## 1. Intended model vs current drift

The original profile intent was: a selected profile expresses role/model/tool/context policy, then the resolver turns that policy into a concrete, validated runtime manifest snapshot. The current implementation drifted into asking profile authors to write `PodManifestConfig` in Nix.

Evidence from the closed `manifest-profiles` work:

- The motivating problem was that low-level manifest knobs such as compaction thresholds and pruning sizes are poor authoring UX; profiles should expose high-level intent and presets.
- The intended runtime boundary was still `selected profile + explicit startup inputs => deterministic resolved manifest/config snapshot => Pod runtime`.
- The Nix profile artifact was meant to be portable and standalone, but the design direction also said semantic presets should be preferred for hard-to-tune values.

Current drift:

- `resources/nix/profile-lib.nix` documents `mkProfile { manifest = mkManifest { ... }; }`; `mkManifest` is currently an identity function.
- `resources/nix/profiles/default.nix` is a manifest-shaped blob under `manifest = insomnia.mkManifest { ... }`.
- The builtin default contains `pod.name = "insomnia"`, which is instance identity, not profile policy.
- Reasoning effort appears as `worker.reasoning = "high"` instead of model/quality policy.
- Compaction values are copied as raw manifest constants (`threshold = 200000`, `request_threshold = 240000`, `worker_context_max_tokens = 100000`) instead of being derived from the selected model's effective context window.
- Builtin default resolution currently goes through `NixProfileResolver`, which shells out to `nix eval` even for the normal no-argument startup path.

The fix should introduce a typed semantic profile artifact and a manifestization step. Nix may remain an authoring language for user/project profiles, but `manifest` must become resolver output, not the public authoring API.

## 2. Current code map

### Profile discovery and selection

- `crates/manifest/src/profile.rs`
  - `ProfileRegistrySource`: `Builtin | User | Project`.
  - `ProfileSelector`: CLI/TUI selector model: explicit path, named source, or default.
  - `ProfileSource`: provenance saved into resolved snapshots; currently path or registry path.
  - `ProfileRegistryEntry`: discovered entry with source/name/path/description/default flag.
  - `ProfileRegistry`: stores entries and one default; implements `default_entry`, `select`, `select_named`, builtin fallback default.
  - `ProfileDiscovery::for_cwd()` wires builtin dir, user `profiles.toml`, nearest project `.insomnia/profiles.toml`.
  - `discover_profile_dir()` registers every builtin `.nix` file or `profile.nix` directory.
  - `load_profile_registry_file()` parses user/project registry TOML; it is selection metadata only, not runtime config.
  - `ProfileSelector::parse_cli()` supports `default`, explicit paths, source-qualified selectors, and path-like compatibility.

### Nix eval and artifact parsing

- `crates/manifest/src/profile.rs`
  - `NixProfileResolver` always uses `std::process::Command` to execute:
    - `nix eval --json --file <absolute-profile-path>`.
  - Missing binary returns `ProfileError::NixUnavailable` with a clear diagnostic.
  - Nonzero status returns `ProfileError::NixFailed` with stderr.
  - JSON stdout is parsed into `serde_json::Value` and passed to `resolve_profile_artifact()`.
  - There is no embedded evaluator and no dependency on `rnix`, `nix-compat`, `tvix`, or similar crates in the workspace `Cargo.toml` files.

### Manifest-shaped artifact parsing / manifestization today

- `crates/manifest/src/profile.rs`
  - `resolve_profile_artifact(source, base_dir, raw_artifact)` currently treats the evaluated artifact as already manifest-shaped.
  - `ProfileEnvelope` only validates optional `profile.format == "insomnia.nix-profile.v1"`.
  - `extract_manifest_value()` accepts:
    - `{ profile = ..., manifest = { ... } }`,
    - `{ profile = ..., config = { ... } }`,
    - or a raw manifest object.
  - It deserializes the extracted value directly as `PodManifestConfig`.
  - It merges `PodManifestConfig::builtin_defaults()`, resolves paths, converts to `PodManifest`, attaches `ProfileManifestSnapshot`, and serializes `manifest_snapshot`.

### Runtime startup path

- `crates/pod/src/main.rs`
  - `resolve_manifest()` defaults to `ProfileSelector::Default` when neither `--profile` nor `--manifest` is provided.
  - `load_profile()` constructs `NixProfileResolver::new().with_workspace_base(cwd)` and calls `resolve()`.
  - `--profile-pod-name` overwrites `resolved.manifest.pod.name` after profile resolution.
  - `--manifest` remains a one-file compatibility/debug path.
  - `load_spawn_config_json()` is an internal typed adopted-spawn path that deserializes `PodManifestConfig` directly.

- `crates/client/src/spawn.rs`
  - `SpawnConfig.profile` is passed to `insomnia-pod --profile`.
  - Fresh profile spawns pass `--profile-pod-name <pod_name>` so profile evaluation and pod-name restore semantics remain separate.

- `crates/tui/src/spawn.rs`, `crates/tui/src/main.rs`
  - TUI uses profile discovery to populate/cycle the fresh-spawn profile field and passes the chosen selector through the client spawn path.

### Manifest and runtime config types

- `crates/manifest/src/config.rs`
  - `PodManifestConfig`: partial manifest/cascade type.
  - `PodMetaConfig.name`: currently required by final `TryFrom<PodManifestConfig> for PodManifest`.
  - `WorkerManifestConfig.reasoning`: low-level worker request setting.
  - `CompactionConfigPartial`: raw numeric compaction fields.
  - `TryFrom<PodManifestConfig> for PodManifest` requires `pod.name` and `scope.allow`, fills defaults, validates absolute paths, and materializes `CompactionConfig`.

- `crates/manifest/src/lib.rs`
  - `PodManifest`: concrete runtime contract; includes `pod`, `model`, `worker`, `scope`, optional `compaction`, `memory`, `web`, `skills`, and profile provenance.
  - `CompactionConfig`: runtime numeric thresholds and budgets.
  - `WorkerManifest.reasoning`: copied into `llm_worker::RequestConfig` by pod code.

### Model catalog and context-window plumbing

- `crates/manifest/src/model.rs`
  - `ModelManifest`: data representation for `model.ref`, inline model fields, auth, capability, `context_window`, `max_context_window`.
  - This crate intentionally does not resolve model refs today.

- `crates/provider/src/catalog.rs`
  - Owns builtin provider/model catalog loading from `resources/providers/builtin.toml` and `resources/models/builtin.toml`.
  - `resolve_model_manifest()` resolves `ModelManifest` into `ModelConfig` with effective `context_window`.
  - Context window resolution order: manifest override > model catalog > provider default > `DEFAULT_CONTEXT_WINDOW`; then clamped by `max_context_window`.
  - Builtin `codex-oauth/gpt-5.5` has `context_window = 1000000`, `max_context_window = 272000`, so effective context window is `272000`.

- `crates/provider/src/lib.rs`
  - Builds live `LlmClient` from resolved `ModelConfig`.

- `crates/pod/src/controller.rs`
  - `build_greeting()` calls `provider::catalog::resolve_model_manifest()` to report effective context window.

- `crates/pod/src/pod.rs`
  - `apply_worker_manifest()` maps `WorkerManifest` into `RequestConfig`; `config.reasoning = wm.reasoning.clone()`.
  - Compaction/memory worker model overrides are resolved at consumer boundary via `provider::build_client()`.

## 3. Proposed semantic profile schema / API shape

Introduce a new artifact format, e.g. `insomnia.semantic-profile.v1`, whose top-level shape is semantic and does not contain `manifest` or `config`.

Suggested JSON shape after Nix evaluation:

```json
{
  "profile": {
    "format": "insomnia.semantic-profile.v1",
    "name": "default",
    "description": "Bundled default Insomnia coding profile"
  },
  "policy": {
    "role": "coder",
    "model": {
      "ref": "codex-oauth/gpt-5.5",
      "quality": "high",
      "reasoning": { "effort": "high" }
    },
    "scope": {
      "workspace": { "permission": "write", "recursive": true }
    },
    "tools": {
      "web": { "enabled": true, "search": { "provider": "brave", "api_key_env": "BRAVE_SEARCH_API_KEY" } }
    },
    "context": {
      "compaction": {
        "preset": "coding-long-context",
        "threshold_ratio": 0.74,
        "request_threshold_ratio": 0.88,
        "worker_context_ratio": 0.37
      }
    },
    "memory": {
      "enabled": true,
      "extract_threshold_ratio": 0.18,
      "consolidation_threshold_files": 5,
      "consolidation_threshold_bytes": 50000
    },
    "session": { "record_event_trace": true }
  }
}
```

The exact field names can change during implementation, but the separation should be strict:

- `profile`: metadata and format only.
- `policy`: semantic profile policy.
- No `pod.name` in `policy`.
- No top-level `manifest` / `config` in semantic v1.
- No `mkManifest` in builtin examples.
- Any raw manifest escape hatch, if kept, should be a separate explicit compatibility/debug resolver path, not `mkProfile`'s normal output.

Suggested Rust types in `crates/manifest/src/profile.rs` or a new `crates/manifest/src/profile/semantic.rs`:

```rust
pub struct SemanticProfileArtifact {
    pub profile: ProfileMetadata,
    pub policy: SemanticProfilePolicy,
}

pub struct SemanticProfilePolicy {
    pub role: Option<ProfileRole>,
    pub model: SemanticModelPolicy,
    pub scope: SemanticScopePolicy,
    pub worker: SemanticWorkerPolicy,
    pub tools: SemanticToolPolicy,
    pub context: SemanticContextPolicy,
    pub memory: Option<SemanticMemoryPolicy>,
    pub session: Option<SemanticSessionPolicy>,
    pub advanced: Option<AdvancedProfileOverrides>,
}
```

Minimum viable semantic fields for this ticket:

- `model.ref`: catalog ref such as `codex-oauth/gpt-5.5`.
- `model.auth`: optional `AuthRef` override or secret ref, preserving current secret-reference behavior.
- `model.quality`: optional named policy (`low|balanced|high|max`) used to derive default reasoning/output/context behavior.
- `model.reasoning`: either named effort (`low|medium|high|xhigh`) or budget ratio/tokens. It becomes `WorkerManifest.reasoning` only during manifestization.
- `scope.workspace`: common workspace permission policy that maps to `scope.allow target = <workspace-base>`.
- `tools.web`: semantic enablement plus provider-specific search config already supported by `WebConfig`.
- `context.compaction`: ratios/presets against the selected model's effective context window.
- `memory`: either disabled/omitted or semantic thresholds, preferably ratios where they are context-window dependent.
- `session.record_event_trace`: acceptable profile policy because it controls session behavior, not instance identity.
- `advanced.manifest_overrides`: optional, if needed, as an explicitly named escape hatch. Do not expose it in builtin profiles or docs as the normal path.

Suggested Nix library surface:

```nix
insomnia.mkProfile {
  name = "default";
  description = "Bundled default Insomnia coding profile";
  role = "coder";
  model = insomnia.models.codexOAuth.gpt55 // {
    quality = "high";
    reasoning = insomnia.reasoning.effort "high";
  };
  scope = insomnia.scopes.workspaceWrite;
  context = insomnia.context.longCoding;
  tools.web = insomnia.web.braveFromEnv "BRAVE_SEARCH_API_KEY";
  memory = insomnia.memory.defaultLongContext;
  session.recordEventTrace = true;
}
```

`resources/nix/profile-lib.nix` should output semantic JSON only. `mkManifest` should be removed from the public/builtin style. If compatibility is retained temporarily, name it something noisy such as `unsafeRawManifestProfile` and do not use it in builtin docs/tests.

## 4. Manifestization design

### New resolver boundary

Add an explicit manifestization function that receives both the semantic artifact and runtime inputs:

```rust
pub struct ProfileRuntimeInputs {
    pub pod_name: String,
    pub workspace_base: PathBuf,
}

pub fn manifestize_semantic_profile(
    source: ProfileSource,
    artifact: SemanticProfileArtifact,
    inputs: &ProfileRuntimeInputs,
    model_catalogs: &dyn ModelCatalogResolver,
) -> Result<ResolvedProfile, ProfileError>;
```

The resolver output remains a concrete `PodManifest` plus serialized `manifest_snapshot`; session restore should continue using the snapshot rather than re-evaluating the profile.

### Pod name / identity

- `pod.name` must be supplied by runtime inputs, not profile policy.
- `insomnia-pod` should pass the effective fresh pod name into profile resolution, not patch the name afterward.
- For CLI no-argument/`--pod` fresh startup, `cli.pod` or a generated/default instance name remains a startup input.
- For TUI spawn, `client::SpawnConfig.pod_name` becomes `ProfileRuntimeInputs.pod_name` through `--profile-pod-name`.
- Builtin profile artifacts should not contain any `pod` section.
- In resolved manifest snapshots, `pod.name` is present because snapshots are runtime artifacts and are used for restore.

Implementation detail: `TryFrom<PodManifestConfig> for PodManifest` can keep requiring `pod.name`; manifestization should populate `PodManifestConfig.pod.name = Some(inputs.pod_name.clone())` before validation.

### Model and reasoning policy

- Semantic `model.ref` maps to `PodManifestConfig.model.ref_`.
- Auth overrides map to `PodManifestConfig.model.auth`.
- Manifestization resolves the model ref against the model catalog once to obtain effective context window and capability.
- `model.reasoning` / `model.quality` maps to `WorkerManifestConfig.reasoning`:
  - If explicit effort is given, produce `ReasoningControl::Effort(...)`.
  - If budget ratio is given and model capability supports token budgets, compute budget tokens from context window.
  - If no explicit reasoning is given, derive from `quality` and model capability; e.g. high quality on effort-capable models -> `high`, high quality on budget-capable models -> a conservative token budget or leave unset until policy is defined.
- Provider-specific request serialization remains in `llm-worker`; the profile resolver should only produce the existing provider-neutral `ReasoningControl`.

Important dependency issue: `crates/manifest` currently cannot call `provider::catalog` because `provider` depends on `manifest`. To derive compaction inside the profile resolver, move the data-only catalog loading/resolution out of `provider` into `manifest` or a small new crate.

Recommended short-term refactor:

- Move `crates/provider/src/catalog.rs` into `crates/manifest/src/model_catalog.rs` or a new `crates/model-catalog` crate.
- Re-export `ModelConfig`, provider/model entries, `resolve_model_manifest`, and `DEFAULT_CONTEXT_WINDOW` from the new location.
- Update `provider` and `pod::controller::build_greeting()` to use the new location.
- Keep live client construction and auth dereferencing in `provider`.

### Context window and compaction policy

Semantic compaction should be derived from the selected model's effective context window.

Suggested policy type:

```rust
pub struct SemanticCompactionPolicy {
    pub enabled: bool,
    pub preset: Option<CompactionPreset>,
    pub threshold_ratio: Option<f64>,
    pub request_threshold_ratio: Option<f64>,
    pub worker_context_ratio: Option<f64>,
    pub retained_tokens: Option<u64>,
    pub final_reserve_ratio: Option<f64>,
    pub overview_preset: Option<OverviewPreset>,
    pub model: Option<SemanticModelPolicy>,
}
```

Manifestization algorithm:

1. Resolve main model to effective `context_window`.
2. Expand preset defaults into ratios and fixed defaults.
3. Compute raw thresholds:
   - `threshold = floor(context_window * threshold_ratio)`.
   - `request_threshold = floor(context_window * request_threshold_ratio)`.
   - `worker_context_max_tokens = floor(context_window * worker_context_ratio)`.
4. Clamp outputs to safe ranges:
   - Ensure `threshold < request_threshold` when both are present; otherwise return a profile validation error instead of silently emitting suspicious config.
   - Ensure `request_threshold < context_window` by reserving at least `final_reserve_tokens` or a minimum fixed reserve.
   - Keep worker context below the main context window and above a minimum useful budget.
5. Fill `CompactionConfigPartial` with computed numeric fields and existing default-backed fields where appropriate.
6. If `compaction.model` is specified semantically, resolve it separately and derive any compactor-specific worker context budget from that model, not from the main model.

For the current builtin default intent using `codex-oauth/gpt-5.5`, effective context window is `272000`. Ratios close to the existing behavior would be approximately:

- proactive threshold: `200000 / 272000 ~= 0.735`.
- request threshold: `240000 / 272000 ~= 0.882`.
- worker context max: `100000 / 272000 ~= 0.368`.

Those should become preset values (e.g. `longCoding`) rather than magic constants in the Nix profile.

### Scope, tools, memory, session

- `scope.workspace` maps to a `ScopeRule` using `inputs.workspace_base`, not the profile file directory for builtin profiles.
- User/project profile relative paths still resolve against the profile file directory for explicit path fields, but semantic workspace scope should be explicit about using the launch workspace.
- `tools.web` can map directly to existing `WebConfig` because its current fields are already policy-like enough for the minimum implementation.
- `memory.extract_threshold` should either remain explicit for now or gain `extract_threshold_ratio`. If ratio exists, derive from the same effective context window.
- `session.record_event_trace` maps to existing `SessionConfigPartial`.

### Snapshot/provenance

- `ResolvedProfile.manifest_snapshot` remains the validated runtime snapshot.
- Consider replacing or narrowing `raw_artifact` retention for semantic profiles. It can remain for diagnostics in tests, but avoid logging or persisting raw Nix output beyond the validated snapshot.
- `ProfileManifestSnapshot` should continue recording source and profile metadata. For builtin semantic profiles, allow a source that does not imply a Nix path if builtin runtime no longer evals Nix.

## 5. Nix evaluator boundary

### Recommended short-term behavior

Builtin/default profiles should not require the external `nix` command during normal runtime.

Implement this by splitting profile evaluation into source-specific paths:

- Builtin profiles:
  - Discovered as builtin profile names for UI/selection.
  - Resolved from an in-process semantic definition, not by `nix eval`.
  - `resources/nix/profiles/default.nix` can remain as the Nix authoring example/smoke artifact, but normal `ProfileSelector::Default` / `builtin:default` should not invoke it.
  - Alternatively, if avoiding duplication is important, generate a checked-in static semantic JSON/TOML artifact from the Nix file at build/release time; runtime should still load the checked-in artifact directly.

- User/project explicit Nix profiles:
  - Continue using external `nix eval --json --file <path>` for now.
  - Keep diagnostics clear: selecting a Nix-authored user/project profile requires the `nix` command unless/until an embedded evaluator is implemented.
  - Add a timeout around `nix eval` as a robustness follow-up or in this ticket if low-risk.

- Explicit non-Nix resolved semantic artifacts:
  - Consider supporting `.json` / `.toml` semantic artifacts as test/debug inputs that bypass Nix entirely. This is useful for tests and for users without Nix.

### Embedded evaluator feasibility notes

Current local code has no embedded Nix evaluator dependency. Adding one is not a small drop-in change because the profile examples rely on imports and Nix language evaluation, not just parsing.

Practical options:

- `rnix`-style parser only: not sufficient; it parses Nix syntax but does not evaluate imports/functions/attribute merges.
- `nix-compat` / `tvix` ecosystem: may provide evaluation building blocks, but integrating an evaluator, file imports, builtins policy, purity constraints, and JSON conversion is a separate design task.
- Custom evaluator for a tiny subset: risky unless the supported subset is extremely small; it would create a second Nix-like language and likely fail on normal Nix idioms.

Recommendation: do not attempt embedded Nix evaluation in this ticket. Isolate external evaluation to user/project Nix-authored profiles, make builtin defaults in-process, and document the boundary.

## 6. Step-by-step implementation phases

### Phase 1: Extract model catalog resolution for manifestization

Likely changed files:

- `crates/provider/src/catalog.rs`
- `crates/provider/src/lib.rs`
- `crates/manifest/src/lib.rs`
- `crates/manifest/src/model.rs` or new `crates/manifest/src/model_catalog.rs`
- `crates/pod/src/controller.rs`

Tasks:

1. Move data-only provider/model catalog types and `resolve_model_manifest` into `manifest` or a new no-cycle crate.
2. Keep provider live client construction in `provider`.
3. Update imports in `provider` and `pod`.
4. Preserve current catalog behavior and tests, including context-window clamping.

### Phase 2: Add semantic artifact/types and manifestization

Likely changed files:

- `crates/manifest/src/profile.rs` or new `crates/manifest/src/profile/semantic.rs`
- `crates/manifest/src/config.rs`
- `crates/manifest/src/lib.rs`

Tasks:

1. Add `SEMANTIC_PROFILE_FORMAT_V1 = "insomnia.semantic-profile.v1"`.
2. Add typed semantic policy structs with serde support.
3. Add `ProfileRuntimeInputs`.
4. Add `manifestize_semantic_profile()`:
   - fill `pod.name` from runtime inputs;
   - map semantic model/ref/auth to `ModelManifest`;
   - resolve model context window;
   - derive reasoning;
   - derive compaction thresholds from ratios/presets;
   - map scope/tools/memory/session;
   - merge `PodManifestConfig::builtin_defaults()` and validate into `PodManifest`;
   - attach profile provenance and serialize snapshot.
5. Make semantic format reject top-level `manifest`/`config`.
6. Keep old v1 manifest-shaped resolver only as an explicit compatibility path if necessary; do not use it for builtin defaults or docs.

### Phase 3: Split builtin profile resolution from external Nix eval

Likely changed files:

- `crates/manifest/src/profile.rs`
- `crates/manifest/src/paths.rs`
- `crates/pod/src/main.rs`
- `crates/tui/src/spawn.rs` if entry metadata needs adjustment

Tasks:

1. Add a builtin profile source representation that can resolve without a path, or mark builtin registry entries with an internal resolver kind.
2. Change `ProfileDiscovery` so builtin `default` is still listed/selectable but does not imply `nix eval`.
3. Add `BuiltinProfileResolver` or `ProfileResolver` enum/trait:
   - builtin semantic definitions -> in-process artifact -> manifestization;
   - user/project/path `.nix` -> `NixProfileResolver` -> semantic artifact -> manifestization;
   - optional `.json`/`.toml` semantic artifact -> parse -> manifestization.
4. Update `load_profile()` to pass the pod name and workspace base into resolution instead of overwriting `manifest.pod.name` afterward.
5. Ensure no-argument `insomnia-pod` and `builtin:default` do not spawn `nix`.

### Phase 4: Replace Nix authoring library and builtin default shape

Likely changed files:

- `resources/nix/profile-lib.nix`
- `resources/nix/profiles/default.nix`
- `docs/manifest-profiles.md`
- `docs/architecture.md`
- `docs/nix.md`
- `docs/pod-factory.md`

Tasks:

1. Rewrite `profile-lib.nix` so `mkProfile` emits `{ profile = { format = "insomnia.semantic-profile.v1"; ... }; policy = ...; }`.
2. Remove `mkManifest` from builtin examples and docs.
3. Provide semantic helper namespaces for model, reasoning, context/compaction, scopes, web, memory, secrets.
4. Rewrite builtin `default.nix` semantically and remove `pod.name`.
5. Update docs to describe semantic profiles and the evaluator boundary.

### Phase 5: Tighten compatibility and diagnostics

Likely changed files:

- `crates/manifest/src/profile.rs`
- `crates/pod/src/main.rs`
- profile-related tests

Tasks:

1. Decide whether old `insomnia.nix-profile.v1` manifest-shaped artifacts are rejected, accepted only via a compatibility flag, or accepted with a deprecation diagnostic.
2. Given the ticket's direction, prefer not preserving awkward authoring compatibility unless the parent explicitly wants a transition period.
3. Improve errors:
   - semantic profile contains `pod.name` -> explain pod identity belongs to startup input;
   - semantic profile contains raw compaction constants in the wrong place -> explain ratio/preset fields;
   - missing `nix` for user/project `.nix` profile -> current diagnostic plus source selector/path;
   - builtin profile resolution should never mention missing `nix`.

### Phase 6: Optional robustness follow-ups if still in scope

- Add timeout to external `nix eval`.
- Avoid retaining `ResolvedProfile::raw_artifact` outside debug/test APIs.
- Add explicit `insomnia-pod --profile-artifact <path.json>` if JSON semantic artifacts prove useful.

## 7. Tests and validation commands

### Tests to add/update

`crates/manifest/src/profile.rs` / semantic module:

- Builtin semantic default manifestizes without `pod.name` in the source artifact and with runtime input `pod_name` in the final `PodManifest`.
- Semantic `model.ref = "codex-oauth/gpt-5.5"` uses catalog-clamped context window `272000`.
- Semantic compaction preset/ratios derive expected thresholds from context window.
- Reasoning effort policy maps to `WorkerManifest.reasoning`.
- Semantic artifact containing top-level `manifest`/`config` or semantic `pod.name` is rejected.
- User/project `.nix` missing binary still returns `NixUnavailable`.
- Builtin default resolution with a missing/fake `nix_bin` still succeeds because builtin does not eval Nix.
- Source-qualified/default/ambiguous discovery behavior remains unchanged for user/project entries.

Model catalog tests:

- Moved catalog tests continue to prove provider/model loading, context-window override, clamp, and unknown provider behavior.
- Add a manifestization test using injected in-memory catalogs if the model catalog resolver is trait-based.

Pod/client/TUI tests:

- `insomnia-pod` no-argument default startup path uses profile default and does not require `nix` for builtin.
- `--profile-pod-name` is passed as runtime input, not a post-resolution patch.
- TUI profile picker still lists `builtin:default` and user/project profiles.
- Existing profile selection tests continue to pass.

Docs/Nix smoke:

- A manual or ignored test can run `nix eval --json --file resources/nix/profiles/default.nix` and assert it emits `insomnia.semantic-profile.v1` with `policy`, not `manifest`.

### Focused validation commands

Run after implementation:

```sh
cargo fmt --check
cargo test -p manifest profile -- --nocapture
cargo test -p manifest model -- --nocapture
cargo test -p provider catalog -- --nocapture
cargo test -p pod --bin insomnia-pod profile -- --nocapture
cargo test -p client spawn -- --nocapture
cargo test -p tui spawn -- --nocapture
cargo check -p session-store -p manifest -p provider -p pod -p client -p tui
nix eval --json --file resources/nix/profiles/default.nix
./tickets.sh doctor
git diff --check
```

If the catalog module moves out of `provider`, adjust package/test filters accordingly.

## 8. Risks and open questions

- **Where to house model catalog resolution:** manifestization needs effective context windows, but current catalog resolution lives in `provider`, which depends on `manifest`. The clean short-term answer is to move data-only catalog resolution into `manifest` or a new crate. Parent should choose whether a new crate is worth it; implementation-wise, moving into `manifest` is smaller.
- **Exact semantic field names:** the schema above is intentionally concrete but not final. The important decision is the boundary: semantic `policy` in, `PodManifestConfig` out.
- **Old manifest-shaped Nix artifacts:** preserving them will keep abstraction leaks alive. Recommendation is to reject them for the new semantic format and keep direct `--manifest` as the low-level escape hatch. If migration is required, make it explicit and temporary.
- **Builtin source provenance:** if builtin profiles no longer resolve from a Nix path, `ProfileSource::Registry { path }` may need to become `ProfileSource::Builtin { name }` or make `path` optional. This is a small schema migration for future snapshots.
- **Reasoning defaults by quality:** `quality = "high"` should not blindly force reasoning for every model. It should consult model capability and either choose an effort/budget policy or leave reasoning unset when unsupported.
- **Compaction ratio defaults:** exact preset ratios need product judgment. The current builtin constants imply ratios around 0.735/0.882/0.368 for `gpt-5.5`; using those as the first `longCoding` preset preserves behavior while moving the authoring surface to semantics.
- **External Nix timeout:** current `Command::output()` can hang indefinitely. This is already a known follow-up; include it in this ticket if touching resolver orchestration is easy, otherwise track separately.
- **Docs currently teach the wrong model:** `docs/manifest-profiles.md`, `docs/pod-factory.md`, `docs/architecture.md`, and `docs/nix.md` should be updated in the same implementation so future work does not copy the manifest-shaped API.
