---
title: 'Workspace Companionを実LLM実行Workerとして起動する'
state: 'inprogress'
created_at: '2026-06-27T18:26:47Z'
updated_at: '2026-06-28T07:05:54Z'
assignee: null
queued_by: 'workspace-panel'
queued_at: '2026-06-27T19:06:32Z'
---

## 背景

Workspace Backend は embedded Runtime 上に `workspace_companion` Worker を自動起動しているが、現時点では実 LLM 実行 backend に接続されていない。そのため Web Console から見ると Companion Worker は存在するが、input-capable Worker として動作しない。過去の providerless / fake response は正規動作ではないため削除済みであり、再導入しない。

この Ticket では、embedded Runtime が既存 `worker` crate 実行に接続された後、Workspace Companion を実際の LLM execution を持つ Worker として起動する。Workspace top の Worker list から Companion Worker を開き、Worker Console で Send すると実 provider / configured model 由来の応答が返る状態を完成させる。

## 目的

- `workspace_companion` Worker が実 LLM 実行に接続されている。
- Web Console から Send すると fake ではない assistant response が返る。
- Companion 専用 chat API / providerless response に依存しない。
- Companion は通常 Worker として Worker list から attach できる。

## 要件

### Companion bootstrap

- Workspace Backend 起動時に、Companion 用 Profile / config bundle を解決して executable Worker を spawn する。
- Companion Worker は `runtime_id + worker_id` で Console attach できる。
- role / profile は `workspace_companion` / appropriate companion Profile として表示される。
- provider / model / secret / prompt / authority が不足している場合は、input-capable Worker として表示せず、typed diagnostic を返す。
- fake / providerless assistant response を生成しない。

### Worker Console behavior

- Workspace top の Worker list から Companion Worker Console を開ける。
- Console composer は実行可能な Companion Worker でのみ有効になる。
- Send は Backend Worker input API を通り、embedded Runtime execution backend 経由で実 Worker run に届く。
- assistant response は provider / Worker execution 由来の `protocol::Event` と transcript に基づいて表示する。
- thinking / tool / status / error が届く場合は既存 Web Console protocol rendering に乗る。

### API cleanup

- Companion 専用 `/api/companion/messages` が残る場合も、正規経路は Worker input API とする。
- `/api/companion/messages` は fake response を返さない。残すなら Worker input API への thin wrapper とし、挙動差を作らない。
- Companion status / transcript は必要な bootstrap/status API として残してよいが、Console の authority は `runtime_id + worker_id` とする。

### Validation / evidence

- Test では real external provider secret を要求しない。deterministic fake provider / test Worker execution backend を使い、fake UI response ではなく Worker execution path を通ったことを確認する。
- Manual smoke では設定済み provider がある環境で Web Console Send -> assistant response -> observation WS event を確認できる。

## Non-goals

- Companion 専用 sidebar entry / standalone Console route の復活。
- providerless canned response の復活。
- Full multi-user auth / permission / redaction policy。
- Advanced Companion UX（global composer、drawer、quick action）。
- remote Runtime 上の Companion 実行。

## 受け入れ条件

- Workspace Backend が executable `workspace_companion` Worker を起動できる。
- provider/config 不足時は fake response ではなく typed diagnostic になり、input-capable Worker として誤表示しない。
- Worker list から Companion Worker Console を開ける。
- Console Send が実 Worker/LLM execution path を通り、assistant response が provider/test provider 由来で表示される。
- `protocol::Event` が Runtime observation bus / Backend WS / Web Console に流れる。
- `/api/companion/messages` が fake response を返さない。残す場合は Worker input API と同一実行経路を使う。
- Focused tests が Companion bootstrap、input dispatch、assistant output、observation event、transcript projection を確認する。
- `cd web/workspace && deno task test` が通る。
- `cd web/workspace && deno task check` が通る。
- `cd web/workspace && deno task build` が通る。
- `cargo test -p yoi-workspace-server` が通る。
- `cargo check -p yoi` が通る。
- `git diff --check` が通る。
- `nix build .#yoi --no-link` が通る。
