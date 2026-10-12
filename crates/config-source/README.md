# config-source

Shares the virtual Workspace configuration source tree, Decodal evaluation, diagnostics, and completion across native/WASM. Inputs are content-addressed snapshots, not a mechanism for reading host configuration directories. Callers own persistence, save-time authorization, and whole-tree CAS.

## Main entry points

- [`VirtualPath`] and [`ConfigTreeSnapshot`] — virtual paths and source-change validation
- [`WorkspaceConfigSchemaBundle`] and [`ToolchainContract`] — evaluation-condition identity
- [`SnapshotEnvironment`] and [`SnapshotImportLoader`] — evaluation and closed import resolution
