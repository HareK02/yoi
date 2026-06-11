---
kind: policy
description: Public workflow resources are procedural artifacts, not prompt fragments or dogfood policy
model_invokation: true
user_invocable: true
last_sources: []
---
Builtin workflow resources live under `resources/workflows` and should contain public, product-generic procedure. Project dogfood details such as repository-specific Git worktree, cargo, nix, merge, and cleanup policy belong in workspace workflows or explicit launch context. Workspace workflow records override builtin workflow resources by slug, and provenance should remain visible.
