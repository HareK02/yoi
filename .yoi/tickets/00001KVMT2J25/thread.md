<!-- event: create author: LocalTicketBackend at: 2026-06-21T10:02:01Z -->

## 作成

LocalTicketBackend によって作成されました。

---

<!-- event: state_changed author: workspace-panel at: 2026-06-21T10:07:42Z from: ready to: planning reason: panel_return_to_planning field: state -->

## State changed

Panel returned ready Ticket 00001KVMT2J25 to planning for requirements sync. This is not Queue routing and must not start implementation.

## User refinement instruction

今あるフロントSPAをnodeからdenoにしたい。
https://hareworks.net/blog/tech/sveltekit-with-deno


---

<!-- event: comment author: ticket-intake at: 2026-06-21T10:09:52Z -->

## Comment

## Intake requirements sync result

対象 Ticket `00001KVMT2J25` の item/thread/artifacts と関連 Ticket/source を確認した結果、今回の refinement instruction はこの Ticket の既存スコープとは一致しないため、`00001KVMT2J25` を ready に戻したり queue routing したりせず、requirements sync の結果としてここに記録する。

### User claims / request snapshot

- ユーザーは「今あるフロントSPAを node から deno にしたい」と述べた。
- 参考 URL として `https://hareworks.net/blog/tech/sveltekit-with-deno` が提示された。

### Confirmed facts / sources

- `00001KVMT2J25` の item は「Attach mid-stream TUI client without losing in-flight content」で、protocol / pod / TUI reconnect / stream-state の実装 Ticket である。
- `00001KVMT2J25` の thread には、Panel が ready から planning に戻した理由として上記 frontend/Deno refinement instruction が記録されている。
- 関連する closed Ticket `00001KVMFFYVX` は Workspace web control plane bootstrap で、SvelteKit static SPA skeleton を `web/workspace` に追加した作業である。
- `web/workspace/README.md` は現在の package manager を npm とし、`npm install`, `npm run check`, `npm run build` を案内している。
- `web/workspace/package.json` は SvelteKit static SPA の scripts / devDependencies を持ち、`package-lock.json` が存在する。
- `web/workspace/svelte.config.js` は `@sveltejs/adapter-static` を使い、`build/` に static output を出す設定である。
- `package.nix` は `web/workspace/node_modules`, `.svelte-kit`, `build` を source filter から除外している。
- `devshell.nix` には `deno` が含まれている。

### Intake conclusion

- この refinement は `00001KVMT2J25` の late-attach / stream-state work item とは別目的であり、同一 Ticket の requirements / acceptance criteria に混ぜると work item が破綻する。
- 既存の frontend bootstrap Ticket `00001KVMFFYVX` は closed であり、現在の npm 採用は当時の bootstrap decision / implementation detail として記録されている。Deno 移行は closed Ticket の再オープンではなく、別の concrete follow-up Ticket として扱うのが妥当。
- 現時点では、ユーザーが「新規 Ticket を作成して」と明示していないため、duplicate/new Ticket は作成しない。

### Candidate follow-up draft if user approves new Ticket

Title: Workspace web SPA の frontend tooling を npm/Node から Deno に移行する

Readiness: implementation_ready
Risk flags: [frontend-tooling, packaging, nix-source-filter, validation]

Requirements:
- `web/workspace` の開発・検証・build 手順を npm/Node 前提から Deno 前提へ移行する。
- SvelteKit static SPA であること、Rust backend が business/API authority を持つこと、static output が `web/workspace/build/` に出ることは維持する。
- `web/workspace/README.md` の package manager / command guidance を Deno 前提に更新する。
- lockfile と依存関係管理を Deno 前提で明確にし、古い npm lockfile/source-of-truth が残る場合は理由を明示する。
- generated artifacts (`node_modules`, `.svelte-kit`, `build` など) が git/Nix package source に混入しない状態を維持する。

Acceptance criteria:
- `web/workspace` の check/build が Deno command で成功する。
- README と project files から、通常の frontend 作業者が Deno で install/check/build できることが分かる。
- npm lockfile / npm scripts / Node-only guidance が不要なら削除され、必要なら Deno 移行後も残す理由が code/docs に明確である。
- `package.nix` / devshell / repository hygiene が Deno 移行後の generated artifacts と矛盾しない。
- 既存の Rust backend static serving assumptions と `.yoi` records as authority の境界を変えない。

Implementation latitude:
- Deno project layout は `deno.json` / `deno.lock` / `deno task` を使う形でよい。
- SvelteKit/Vite を Deno の npm compatibility で動かすか、最小限の package metadata を残すかは実装時に選んでよい。ただし source-of-truth と command guidance は曖昧にしない。

Validation:
- `cd web/workspace && deno task check`
- `cd web/workspace && deno task build`
- `git diff --check`
- packaging/source-filter を変更する場合は `nix build .#yoi --no-link` または Orchestrator が妥当と判断する同等確認

Next routing recommendation:
- `00001KVMT2J25` はこの refinement では implementation_ready に戻さない。
- ユーザーが Deno 移行の新規 Ticket 作成を承認した場合、上記 draft を別 Ticket として materialize する。
- ユーザーがあくまで `00001KVMT2J25` の差し替えを望む場合は、既存 Ticket を repurpose するのではなく、Orchestrator/maintainer に cancel/close/replacement 方針の判断を戻す。

---

<!-- event: intake_summary author: ticket-intake at: 2026-06-21T10:23:54Z -->

## Intake summary

ユーザーから、Deno 移行 refinement は `00001KVMT2J25` に誤って付いたものであり、`00001KVMT2J25` は ready に戻してよいとの明示指示があった。再確認したところ、`00001KVMT2J25` の body は late attach / in-flight stream snapshot の concrete work item で、readiness は `implementation_ready`、blocking open question はない。Deno 移行は別 Ticket `00001KVMV03QY` として作成済みであり、本 Ticket の要件には混ぜない。

---

<!-- event: state_changed author: ticket-intake at: 2026-06-21T10:23:54Z from: planning to: ready reason: requirements_sync_resolved field: state -->

## State changed

Deno 移行 refinement は誤付与として分離済み。`00001KVMT2J25` は元の protocol reconnect / unfinished block snapshot Ticket として Orchestrator routing 可能な ready 状態へ戻す。queue routing や implementation start は行わない。

---
