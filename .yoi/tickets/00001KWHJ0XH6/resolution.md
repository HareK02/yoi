Workspace Browser に Settings/Admin shell と navigation を追加し、manual fallback review 後に orchestration branch へ merge した。

実装内容:
- `/settings` route を追加。
- Sidebar header の gear link と Sidebar 内 Settings section から Settings / Admin へ移動できるようにした。
- Settings / Admin shell を追加し、以下の sections を配置。
  - Runtime Connections（placeholder）
  - Backend Config（placeholder）
  - Workspace Identity（read-only）
- Runtime Connections / Backend Config は明確な placeholder とし、Runtime connection add/delete/test/persist、Backend config editor、secret UI、settings mutation API は実装していない。
- Workspace Identity は opaque workspace id / display name / record authority context の read-only 表示に限定し、raw filesystem path を出していない。
- browser user / role / permission / multi-user authorization model は存在しないこと、admin role を作らないことを明記。
- sanitized diagnostics / restart-required / read-only until typed APIs exist の表示 pattern を追加。
- Settings shell model tests を追加し、navigation、no fake browser admin model、placeholder boundaries、raw authority leak avoidance を確認。
- Existing Worker Console / Sidebar route は維持。

Integrated commit:
- `c0c6880b1a00ec367910267a3d2a0595839b3d5b feat: add settings admin shell`
- merge: `fdad94af merge: settings admin shell`

Validation:
- `deno run -A npm:@sveltejs/kit@2.49.4 sync`: success
- `cd web/workspace && deno task test`: success (`10 passed`)
- `cd web/workspace && deno task check`: success
- `cd web/workspace && deno task build`: success
- `git diff --check`: success

Operational note:
- Coder/Reviewer Pod spawning was unavailable in the current Orchestrator process because `SpawnPod` tried to execute `/home/hare/.cargo/bin/yoi (deleted) pod`. This narrow web-only Ticket was implemented and reviewed directly by Orchestrator as a recorded fallback.
- User instructed not to use `StopPod` for now after repeated stop/hang symptoms. Cleanup will remove only the child implementation worktree / branch.