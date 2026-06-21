---
title: 'Workspace web SPA の frontend tooling を npm/Node から Deno に移行する'
state: 'inprogress'
created_at: '2026-06-21T10:18:10Z'
updated_at: '2026-06-21T11:05:28Z'
assignee: null
readiness: 'implementation_ready'
risk_flags: ['frontend-tooling', 'packaging', 'nix-source-filter', 'validation']
queued_by: 'workspace-panel'
queued_at: '2026-06-21T10:56:31Z'
---

## User claims / request snapshot

- ユーザーは「今あるフロントSPAを node から deno にしたい」と述べた。
- 参考 URL として `https://hareworks.net/blog/tech/sveltekit-with-deno` が提示された。
- この依頼は、Panel から planning に戻された `00001KVMT2J25` の requirements sync 中に追加された。

## Confirmed facts / sources

- `00001KVMT2J25` は protocol / pod / TUI reconnect / stream-state の Ticket であり、frontend tooling 移行とは別目的である。
- `00001KVMT2J25` の thread に、今回の refinement は別 follow-up Ticket として扱うべきことを Intake comment として記録済み。
- Closed Ticket `00001KVMFFYVX` は Workspace web control plane bootstrap で、SvelteKit static SPA skeleton を `web/workspace` に追加した。
- `00001KVMFFYVX` の resolution は、frontend が npm + committed `package-lock.json` を使い、generated `node_modules/`, `.svelte-kit/`, `build/` を ignore/source-filter する方針だったことを記録している。
- 現在の `web/workspace/package.json` は npm scripts と SvelteKit/Vite devDependencies を持つ。
- 現在の `web/workspace/package-lock.json` は npm lockfile として存在する。
- 現在の `web/workspace/README.md` は package manager を npm とし、`npm install`, `npm run check`, `npm run build` を案内している。
- 現在の `web/workspace/svelte.config.js` は `@sveltejs/adapter-static` を使い、`web/workspace/build/` に static output を出す。
- 現在の `package.nix` は `web/workspace/node_modules`, `web/workspace/.svelte-kit`, `web/workspace/build` を source filter から除外している。
- 現在の `devshell.nix` には `deno` が含まれている。
- 参考 URL の内容は untrusted web content として確認した。記事は SvelteKit を Deno task / npm compatibility / `deno.json` ベースで運用する例、Svelte LSP 用 `tsconfig.json` が残りうる点、Deno 用 adapter への言及を含む。

## Unverified hypotheses

- `web/workspace` は static SPA skeleton なので、Deno 移行は backend/API authority を変えず frontend tooling の範囲に収められる可能性が高い。
- SvelteKit/Vite/svelte-check は Deno の npm compatibility と `deno task` で動かせる可能性が高い。
- Svelte LSP / svelte-check のために `tsconfig.json` を完全削除できない可能性がある。
- Deno 移行後も `node_modules` 相当の local generated state が発生する可能性があり、ignore/source-filter の再確認が必要。

## Undecided points / open questions

- blocking な未決定点はなし。
- `package.json` を完全に削除できるか、SvelteKit/Vite ecosystem 互換のため最小限残すかは実装調査で判断してよい。ただし source-of-truth と command guidance を曖昧にしない。
- `tsconfig.json` を維持するか `deno.json` へ寄せるかは、Svelte LSP / svelte-check の実動作に基づいて判断してよい。
- `@sveltejs/adapter-static` を維持するか Deno 向け adapter へ変えるかは、Rust backend が static assets を serve する現方針と矛盾しない範囲で判断してよい。

## Background

Workspace web control plane の frontend は `web/workspace` にある SvelteKit static SPA skeleton である。bootstrap 時点では npm + `package-lock.json` を採用したが、ユーザーは既存 frontend SPA の tooling を Node/npm 前提から Deno 前提へ移行したい。

この Ticket は frontend tooling / package-manager migration の concrete follow-up であり、protocol reconnect Ticket `00001KVMT2J25` とは別 work item として扱う。

## Requirements

- `web/workspace` の開発・検証・build 手順を npm/Node 前提から Deno 前提へ移行する。
- Deno 側の project configuration を明確にする。候補は `deno.json` または `deno.jsonc`、`deno.lock`、`deno task`。
- SvelteKit static SPA であることを維持する。
- Rust backend が business/API authority を持ち、frontend は lifecycle/business authority を持たない境界を維持する。
- static build output が backend から serve できる形を維持する。既存の `web/workspace/build/` を変える場合は backend/README/package hygiene と整合させる。
- `web/workspace/README.md` の package manager / command guidance を Deno 前提に更新する。
- lockfile と依存関係管理を Deno 前提で明確にする。
- 古い npm lockfile / npm scripts / Node-only guidance が不要なら削除し、残す必要がある場合は理由を project files または docs に明記する。
- generated artifacts (`node_modules`, `.svelte-kit`, `build` など) が git / Nix package source に混入しない状態を維持する。
- `package.nix` source filtering と devshell/package guidance が Deno 移行後の frontend generated state と矛盾しないことを確認する。

## Acceptance criteria

- `web/workspace` の check が Deno command で成功する。
- `web/workspace` の build が Deno command で成功する。
- README と project files から、通常の frontend 作業者が Deno で install/check/build できることが分かる。
- npm lockfile / npm scripts / Node-only guidance が不要なら削除されている。
- npm/Node 関連ファイルを残す場合、そのファイルが canonical source-of-truth なのか compatibility artifact なのかが明確である。
- SvelteKit static SPA と Rust backend static serving の前提が壊れていない。
- `web/workspace/build/`, `.svelte-kit/`, `node_modules` または Deno 移行後の同等 generated artifacts が git/Nix package source に混入しない。
- 既存の `.yoi` records as authority、Ticket/Objective workflow、Rust backend API authority を変更しない。

## Binding decisions / invariants

- この Ticket は frontend tooling migration であり、Workspace backend の API authority / Ticket lifecycle authority / `.yoi` canonical records を変更しない。
- Frontend を SSR authority や business/lifecycle authority にしない。
- Static SPA と Rust backend serving の境界を維持する。
- `00001KVMT2J25` は protocol reconnect Ticket として残し、本 Ticket に混ぜない。
- Tooling source-of-truth を npm と Deno の二重管理で曖昧にしない。
- Generated artifacts を committed source や Nix package source に混入させない。

## Implementation latitude

- Deno project layout は `deno.json` / `deno.lock` / `deno task` を使う形でよい。
- SvelteKit/Vite を Deno の npm compatibility で動かすか、最小限の package metadata を残すかは実装時に選んでよい。
- `tsconfig.json` は Svelte LSP / svelte-check の都合で必要なら残してよい。
- `@sveltejs/adapter-static` を維持してよい。Deno adapter を採用する場合は、この workspace の Rust backend static serving 方針と矛盾しないことを確認する。
- `package.nix` / devshell / README の更新範囲は、Deno 移行後の generated artifact と validation command に合わせて最小限でよい。

## Readiness

- readiness: implementation_ready
- risk_flags: [frontend-tooling, packaging, nix-source-filter, validation]

## Escalation conditions

- Deno だけでは SvelteKit check/build が安定せず、Node/npm を primary に残す必要が出た場合。
- `package.json` / `package-lock.json` を残すか削除するかが project policy decision になる場合。
- Static SPA ではなく SSR / Deno runtime server / Deno Deploy 前提へ移る必要が出た場合。
- Rust backend static serving path や package/Nix build 方針の大きな変更が必要になった場合。
- Generated artifacts の source-filter / ignore 境界が不明確になる場合。

## Validation

- `cd web/workspace && deno task check`
- `cd web/workspace && deno task build`
- `git diff --check`
- frontend generated artifacts が ignored / source-filtered されていることの確認。
- packaging/source-filter を変更する場合は `nix build .#yoi --no-link` または Orchestrator が妥当と判断する同等確認。
- 必要に応じて `cargo check -p yoi-workspace-server` または static serving 周辺の focused tests。

## Related work

- `00001KVMT2J25` — protocol reconnect / in-flight stream snapshot Ticket。今回の Deno 移行とは別件。
- `00001KVMFFYVX` — Workspace web control plane bootstrap。`web/workspace` の SvelteKit static SPA skeleton と npm/package-lock 採用元。
- Relevant files:
  - `web/workspace/package.json`
  - `web/workspace/package-lock.json`
  - `web/workspace/README.md`
  - `web/workspace/svelte.config.js`
  - `web/workspace/vite.config.ts`
  - `web/workspace/tsconfig.json`
  - `web/workspace/.gitignore`
  - `package.nix`
  - `devshell.nix`
