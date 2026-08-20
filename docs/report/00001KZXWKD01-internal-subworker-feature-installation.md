# Internal SubWorker feature installation fails before analysis starts

While implementing Ticket `00001KZXWKD01`, two read-only Internal SubWorkers were requested to investigate the backend and Web Console paths. Both `SubWorkerSpawn` operations failed before the child session started with:

```text
install Internal Worker features: Worker feature installation failed:
builtin:worker-observation: required service requirement is not available:
builtin:worker.control
```

The requested `builtin:coder` child had read-only scope and did not need peer Worker observation for the delegated investigation. The failure prevented context splitting, so the parent Worker performed the investigation directly. No implementation or validation authority was lost.

## Improvement direction

Resolve the effective Internal SubWorker Profile so its installed feature set is satisfiable under the parent-provided services. Either install the required `worker.control` service before `worker-observation`, or avoid enabling `worker-observation` for a child that has no corresponding observation grant/service. Startup validation should identify the Profile feature that introduced the unsatisfied dependency and distinguish a configuration error from unavailable delegated authority.
