# T-711: nested lazy-object import cycle

Date: 2026-10-06
Status: fixed at the snapshot import-loader boundary; regression coverage retained.

## Dogfooding problem

During T-711 builtin-import validation, a nested import cycle in a lazy object caused a stack overflow instead of a recoverable configuration diagnostic. A configuration author could therefore terminate evaluation while writing an invalid recipe, rather than receive an error and keep the active revision. This report records the implementation-session failure and the guard added by the parent config-source implementation; it is not a recommendation to rerun the crashing version inside a live Yoi Worker.

## Minimal reproducer

Build a `ConfigTreeSnapshot` containing these two Decodal entries:

`main.dcdl`:

```dcdl
{
    builtin = import "$builtin/profiles/companion.dcdl";
    cycle = import "./cycle.dcdl";
}
```

`cycle.dcdl`:

```dcdl
import "./main.dcdl"
```

Evaluate the snapshot with `SnapshotEnvironment::evaluate_contract` and a `ToolchainContract` whose entrypoint is `main.dcdl`. This fixture intentionally uses the plain evaluation contract rather than `WorkspaceConfigSchema`: it isolates import/materialization behavior, not Profile registration semantics.

Before the loader guard, evaluating the root object could defer its `cycle` field until materialization. Importing the root again through `cycle.dcdl` then recursed through that lazy value instead of returning a structured cycle failure. The observed failure was a process stack overflow. Increasing the stack would only postpone the failure and would not provide a safe configuration boundary.

## Fix

`SnapshotImportLoader::load` now records canonical source-to-source edges and validates the observed graph **before returning a source to Decodal**. Content-digest cache suffixes are stripped for graph identity, so the same virtual source cannot evade cycle detection by appearing under a cache key.

- A back edge returns a `cycle` diagnostic before lazy materialization can recurse.
- An import chain longer than 32 edges returns an import-depth limit diagnostic. The synthetic evaluation wrapper's entrypoint import counts toward that limit.
- Memoized dependency depths keep shared-subgraph validation bounded without losing the longest path.
- Rejected edges are removed from the successful-edge cache. A reused language-service loader must reject the same bad edge again, not treat it as already validated.
- Workspace and builtin imports use the same graph guard and shared source count/byte budgets. Embedded sources are not an exception.

The fix belongs in `crates/config-source/src/lib.rs`; it does not require runtime Decodal evaluation. Project launch archives still transport the evaluated JSON entrypoint with builtin source provenance, and Runtime resolves only that evaluated value.

## Regression evidence

The following tests in `crates/config-source/src/lib.rs` cover the incident and adjacent guard behavior:

- `builtin_values_do_not_bypass_cycle_or_path_limits`: the two-entry lazy-object fixture above returns `kind = "cycle"`; builtin imports do not bypass depth/path bounds.
- `rejected_import_graph_edges_stay_rejected_after_diagnostics`: repeated attempts to add the rejected back edge remain errors when the loader is reused.
- `import_graph_shared_dependencies_are_memoized_without_losing_depth`: shared dependencies preserve longest-path validation and the allowed depth boundary.
- `snapshot_import_depth_is_checked_before_evaluation_recurses`: a real source chain fails with an import-depth diagnostic before evaluation recurses beyond the guard.
- `builtin_dependencies_share_snapshot_count_and_byte_budgets`: builtin dependencies cannot exceed the aggregate entry-count or byte budget by living outside the authored tree.

Run individual regressions with the normal narrow target, for example:

```sh
cargo test -p config-source --lib builtin_values_do_not_bypass_cycle_or_path_limits
cargo test -p config-source --lib snapshot_import_depth_is_checked_before_evaluation_recurses
```

The server follow-up test `value_profiles_builtin_http_config_tree_is_read_only_and_never_falls_back` separately exercises authenticated HTTP commits: reserved builtin mutations and unknown imports are rejected without changing the active revision, even when a Workspace file exists at the unknown builtin's suffix. Editor import completions remain covered by the WASM editor tests; no server completion endpoint was introduced.

## Operational lesson

An evaluator's module cache is not sufficient evidence that lazy-value cycles are safe. Keep cycle and depth validation at the source-loader boundary and retain the lazy-object reproducer whenever import identity, caching, or materialization changes. Config errors should be recoverable diagnostics, never an implicit request to replace a running Worker's persisted Manifest or restart the live dogfooding process.
