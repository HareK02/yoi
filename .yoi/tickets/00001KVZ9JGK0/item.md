---
title: 'Backend内蔵Companion RuntimeとWeb Console MVP'
state: 'planning'
created_at: '2026-06-25T11:45:17Z'
updated_at: '2026-06-25T20:23:10Z'
assignee: null
queued_by: 'workspace-panel'
queued_at: '2026-06-25T16:44:45Z'
---

## 背景

Workspace backend は Worker runtime registry / Backend internal runtime を control plane として扱う方向に進んでいる。Orchestrator については Backend internal runtime 上の Worker として Kanban / Ticket event を routing する設計が固まりつつある。同じ考え方で、Companion も local Pod / TUI 専用ではなく、Backend internal runtime 上の lightweight Worker として起動し、Web frontend から接続できるようにしたい。

この Ticket では、TUI Console の Web 移植版に向けた MVP として、Backend internal Companion Worker にメッセージを送り、LLM 応答を Web frontend で受け取るところまでを実装する。Companion は v0 では filesystem / shell / ticket mutation / runtime operation tools を持たなくてよい。まずは tools なしの conversational Worker として、Backend internal runtime、Web API、Web console UI、stream / transcript projection の最小経路を作る。

## 目的

- Backend internal runtime 上で Companion Worker を起動・保持できる。
- Workspace web frontend から Companion に接続できる。
- Web console UI から message を送信し、Companion の応答を表示できる。
- TUI Console の基本体験を Web に移植するための最小 transcript / run status / input path を作る。
- v0 では tool authority を持たせず、Backend internal conversational Worker として安全に始める。

## 要件

### Backend internal Companion runtime

- Backend internal runtime 上に Companion Worker を表現する。
- Companion は local Pod process / Unix socket / `.yoi/pods` metadata に依存しない。
- Worker identity は runtime scoped に扱う。
  - `runtime_id`
  - `worker_id`
  - `display_name`
  - `display_ref` 例: `companion@backend-internal`
- Runtime registry / Worker list/detail から Backend internal Companion が見える。
- v0 Companion は tools なし、または明示的に empty tool registry / minimal safe tool registry とする。
- Workspace filesystem、shell、git、Ticket mutation、raw session path、raw socket path を Companion authority にしない。

### Conversation / transcript model

- Backend internal Companion に user message を送れる API を追加する。
- Assistant response を Web frontend が受け取れるようにする。
- v0 は以下のどちらかの方式でよい。
  - request / response 完了後に transcript を返す。
  - SSE / streaming endpoint で delta / final response を返す。
- 実装方式は実装時に選んでよいが、UI が「送る -> 返る」を確認できること。
- Backend は raw provider trace を durable authority にしない。
- Web console 用 transcript は bounded projection とし、将来 prune / overview 化できる形にする。
- usage aggregate / run status は取れる範囲で残す。v0 で詳細 dashboard は不要。

### Web API

- Workspace server に Companion connection / message API を追加する。
- API は browser から raw runtime path / socket path / session path を受け取らない。
- API は current workspace の Backend internal Companion を解決する。
- 最低限以下を扱う。
  - Companion status / detail 取得。
  - Transcript / conversation projection 取得。
  - User message 送信。
  - Assistant response 取得または stream。
- Error は typed response として扱う。
  - companion unavailable
  - already running / busy
  - invalid input
  - provider error
  - response timeout / cancelled

### Web Console UI

- Workspace web に Companion Console 画面または panel を追加する。
- TUI Console の基本 UI を Web 向けに移植する。
  - transcript 表示。
  - user message composer。
  - sending / generating / idle / error 状態表示。
  - assistant response の表示。
- v0 は message round-trip が主目的であり、TUI Console の全機能移植は不要。
- tool call UI、file viewer、diff viewer、thinking block grouping、multi Pod attach は scope 外でよい。
- Web UI は Backend API response / stream を authority とし、local session file / Pod socket を直接読まない。

### Runtime / LLM integration

- Backend internal Companion は existing LLM worker / provider config / profile selection のどれを使うか実装時に決める。
- v0 では project/default Companion profile の完全継承は必須ではないが、model / provider / language / prompt selection の最小方針を明確にする。
- Companion prompt は Rust 直書きではなく prompt resource boundary を使う。
- tools なし Companion でも system prompt / conversation history / current workspace identity は最小限渡せるようにする。
- Long-running provider request 中に複数 message を送った場合の扱いを決める。
  - v0 は single-flight / busy reject でよい。

### Safety / authority

- Browser は raw provider credential、socket path、session path、runtime file path を知らない。
- Backend internal Companion は workspace filesystem / shell / git / Ticket mutation authority を持たない。
- 将来 tool を追加する場合も、domain-specific backend operation / explicit grant 経由にする。
- User message / assistant response は normal conversation history として扱い、hidden context injection にしない。
- Provider error / cancellation / timeout は Web UI に明示する。

## Non-goals

- Full TUI Console parity。
- Tool call execution UI。
- Filesystem / shell / git / Ticket mutation tools を Companion に渡すこと。
- Local Pod Companion の廃止。
- Remote runtime implementation。
- Multi-user auth / permission model の完成。
- Persistent raw session DB ingest。
- Usage dashboard の完成。
- Orchestrator routing / Kanban integration。

## 受け入れ条件

- Backend internal runtime 上に Companion Worker が存在し、runtime / worker API から確認できる。
- Web frontend から Backend internal Companion の status / transcript projection を取得できる。
- Web frontend の Console UI から user message を送信できる。
- Companion が LLM response を生成し、Web UI に表示される。
- v0 Companion は filesystem / shell / git / Ticket mutation tools を持たない。
- Browser が raw socket path / session path / runtime path / provider credential を扱わない。
- Provider request 中の busy / error / timeout が typed error または UI state として扱われる。
- Prompt prose は resource boundary に置かれている。
- Focused backend / frontend tests が追加されている、または E2E 不足の場合はテスト可能範囲と手動確認手順が記録されている。
- `cargo test -p yoi-workspace-server` が通る。
- `cargo check -p yoi` が通る。
- `cd web/workspace && deno task check && deno task build` が通る。
- `git diff --check` が通る。
- `nix build .#yoi --no-link` が通る。
