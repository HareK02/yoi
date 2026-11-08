# manifest

Validates reusable Worker Profiles and resolves them into Manifests with launch-time names, paths, and Feature composition. Host operation grants are not Profile configuration. This crate owns configuration types and resolution boundaries, not Worker execution or credential retrieval.

## Main entry points

- [`ProfileResolver`] / [`ResolvedProfile`] — Profile selection and resolution results
- [`WorkerManifestConfig`] / [`WorkerManifest`] — Partial configuration merging and execution contracts
- [`Scope`] / [`DelegationScope`] / [`SharedScope`] — Direct access and child delegation checks
- [`plugin`] — Offline package inspection, independent of execution authority
