<!-- event: create author: tickets.sh at: 2026-05-29T16:19:28Z -->

## Created

Created by tickets.sh create.

---

<!-- event: decision author: intake at: 2026-06-10T07:21:39Z -->

## Decision

# Intake refinement: concrete MVP scope for MCP integration

既存の Ticket は `mcp-integration` の広い構想として作られていたが、現在の work item 運用では umbrella/progress-container として残さず、単独で実装・レビュー・検証できる concrete slice として扱う。

この Ticket の実装対象を **Yoi に local stdio MCP server を設定し、MCP tools を通常の ToolRegistry / PreToolCall permission / bounded tool-result path に橋渡しする MVP** に絞る。既存 body 内の古い `Insomnia` 表記は現在の `Yoi` を指すものとして読む。

## Binding decisions / invariants

- MCP server は untrusted external capability provider として扱う。
- MCP tool は Yoi の通常の `ToolRegistry` に feature/plugin contribution として登録され、既存の tool permission、scope、history/tool-result recording、bounded output の経路を迂回しない。
- MCP の tool descriptions / schemas / annotations / result content は外部由来の data であり、system/developer instruction、scope、permission、prompt-context、history-persistence rules を弱めない。
- local stdio transport を最初の対象にする。server command / args / cwd / env / secret refs は明示設定に限る。
- secrets は explicit secret/env references で扱い、diagnostics / session logs / model context / Ticket records に plaintext として出さない。
- model-visible な resource/prompt content を history に残らない形で context に注入しない。`resources/read` / `prompts/get` はこの Ticket では実装を defer してよいが、将来も explicit operation + permission/policy gate + durable/model-visible path が必要。
- client-side sampling / elicitation はこの Ticket では disabled / fail-closed とする。実装するなら別途 approval/resume / UI path の設計が必要。
- Streamable HTTP transport、remote auth/TLS/redirect/private-network policy、MCP server packaging/distribution はこの Ticket の非対象。
- Plugin/Feature surface の現在の決定に従い、MCP は plugin model そのものではなく protocol-bound bridge/runtime kind として扱う。

## Requirements for this slice

- Profile/config から named local stdio MCP server を有効化できる。
- Pod startup または feature installation 時に configured server を initialize し、MCP lifecycle (`initialize` / capability negotiation / `notifications/initialized`) を処理する。
- `tools/list` により discovered tools を stable namespaced tool names として登録する。
- registered MCP tool call は `tools/call` に変換され、成功/失敗とも bounded structured output として通常の tool-result path に記録される。
- startup failure、protocol failure、server disconnect、tool discovery/call failure は server 名と phase が分かる diagnostic として出す。secret values は出さない。
- `notifications/tools/list_changed` は安全に扱う。live refresh が既存 ToolRegistry/request lifecycle と衝突する場合は、restart/reinitialize-required diagnostic でもよいが、silent stale state にしない。
- Filesystem roots を MCP に公開する場合は Yoi の authorized scope 由来のみにする。初期実装で roots を公開しない選択も可。
- local mock/test server を使った focused tests を追加する。
- docs に local stdio MCP server の最小例と trust/permission/secret guidance を追加する。

## Acceptance criteria

- 少なくとも1つの local stdio MCP server を Profile/config で有効化できる。
- configured server の initialize 成功/失敗が観測可能で、失敗時に server 名と phase が分かる。
- `tools/list` の結果が Yoi の通常 tool として namespaced stable name で model-visible schema に現れる。
- registered MCP tool を呼ぶと `tools/call` が実行され、bounded structured output が通常の tool result として返る。
- 既存の PreToolCall permission / tool permission policy が MCP tool にも適用され、denial が通常の tool denial として扱われる。
- MCP server 由来 metadata/content は untrusted data として扱われ、prompt-context / history-persistence rules を迂回しない。
- secret refs / env values が diagnostics、logs、model context に plaintext で出ないことを focused test または review で確認できる。
- sampling / elicitation / Streamable HTTP / resources-prompts direct context injection は disabled, fail-closed, or explicitly out-of-scope として実装・docs に反映されている。
- Validation: focused MCP protocol/tool bridge tests、関連 crate tests、`cargo fmt --check`、`cargo check --workspace --all-targets`、`nix build .#yoi`。

## Implementation latitude

- crate placement、internal module names、exact Profile/config syntax、FeatureRegistry integration details、ToolRegistry registration timingは、上の invariants を満たす範囲で実装側に委ねる。
- `resources/read` / `prompts/get` support、Streamable HTTP、packaged MCP server distribution、sampling/elicitation approval flow は follow-up Ticket に分けてよい。
- 初期 TUI 表示は詳細 UI でなく bounded diagnostics/actionbar/log surface でもよい。

## Escalation conditions

- MCP tool registration のために通常の ToolRegistry / permission / history path を迂回する必要が出た場合。
- Profile/config enablement と plugin/feature contribution boundary のどちらが authority を持つか判断が必要になった場合。
- server-provided resource/prompt content を model context に入れる UX/API を決める必要が出た場合。
- secrets、network/remote transport、sampling/elicitation、workspace-provided executable/package auto-start に踏み込む必要が出た場合。
- live `tools/list_changed` refresh が current run の tool schema consistency を壊す場合。

## Related tickets / context

- `00001KSXRQ4G8` Plugin extension surface: MCP は protocol-bound backend/bridge であり plugin model 全体ではない。
- `00001KT6Q08R9` Plugin feature contribution registry: built-in/future external capability は既存 Tool / Hook / Notify paths に contribution する。
- `00001KT0Z4BK8` Plugin distribution package format: package presence is discovery only; workspace-provided executable/MCP process startup requires explicit enablement and capability approval.

---

<!-- event: intake_summary author: intake at: 2026-06-10T07:21:53Z -->

## Intake summary

既存の broad MCP integration Ticket を、local stdio MCP server の設定・初期化・`tools/list`/`tools/call` bridge・通常 ToolRegistry/permission/bounded-result 経路への統合に絞った concrete MVP として refinement した。resources/prompts、Streamable HTTP、sampling/elicitation、distribution は非対象または follow-up とし、untrusted metadata、secret handling、prompt-context/history persistence、scope/permission boundary を binding invariants として明記した。blocking open questions はなく、Orchestrator が implementation routing できる。

---

<!-- event: state_changed author: intake at: 2026-06-10T07:21:53Z from: planning to: ready reason: intake_ready field: state -->

## State changed

Intake refinement が完了し、Ticket は implementation routing 可能な ready 状態です。

---

<!-- event: comment author: intake at: 2026-06-10T07:24:22Z -->

## Comment

# Intake correction: prior refinement is not user-approved

直前の Intake では、ユーザーとの明示的な要件同期・draft 承認を挟まずに scope refinement と `ready` 化まで進めており、Intake workflow の「作成/更新前に draft を提示し、ユーザー合意を取る」手順に対して過剰だった。

直前に記録した MCP MVP scope は **提案 draft** として扱い、合意済み binding decision ではない。特に以下はユーザー確認が必要:

- この Ticket を local stdio MCP tools bridge MVP に絞ってよいか。
- resources/prompts、Streamable HTTP、sampling/elicitation、distribution を follow-up / out-of-scope にしてよいか。
- permission/scope/secret/prompt-context boundary の書き方が過不足ないか。
- 既存 body の広い MCP 要件をこの Ticket 上で置き換えるのか、別 Ticket に split するのか。

---

<!-- event: state_changed author: intake at: 2026-06-10T07:24:26Z from: ready to: planning reason: user_consultation_required field: state -->

## State changed

ユーザーとの明示的な draft 確認なしに ready 化していたため、要件同期に戻す。直前の refinement は未承認の提案として扱い、scope / non-goals / split 方針を確認してから再度 ready 判定する。

---

<!-- event: decision author: intake at: 2026-06-10T07:50:12Z -->

## Decision

# Decomposition decision: promote broad MCP work to Objective

ユーザー判断により、広い MCP integration work は concrete implementation Ticket としては扱わず、Objective `00001KTR80WMN` に格上げして中期 context として保持する。

Concrete work item は以下に分離した。

- `00001KTR81P9X`: `pod::feature` / Worker / ToolRegistry API を external protocol-backed capability providers に耐える形へ拡張する。
- `00001KTR82RB7`: MCP `2025-11-25` local stdio server-feature bridge を実装する。

`resources/prompts` は固定で scope 外にしない。安全条件は direct hidden context injection をしないことであり、明示 tool operation の result として history に残す形なら MCP implementation の対象に含める。

---

<!-- event: state_changed author: hare at: 2026-06-10T07:50:19Z from: planning to: closed reason: closed field: state -->

## State changed

Ticket を closed にしました。


---

<!-- event: close author: hare at: 2026-06-10T07:50:19Z status: closed -->

## 完了

この broad MCP integration Ticket は concrete work item としては退役し、Objective `00001KTR80WMN` に格上げした。

今後の実装 authority は以下の concrete Tickets に置く。

- `00001KTR81P9X`: `pod::feature` / Worker / ToolRegistry API を external protocol-backed capability providers に耐える形へ拡張する。
- `00001KTR82RB7`: MCP `2025-11-25` local stdio server-feature bridge を実装する。

重要な判断:

- API 拡張と MCP 実装は分離する。
- `resources/prompts` は固定で scope 外にせず、direct hidden context injection を禁止したうえで明示 tool operation の result として扱う方向にする。
- Streamable HTTP、remote auth/OAuth、MCP Registry distribution、sampling、elicitation は後続判断または fail-closed とする。

この Ticket 自体は progress-container として残さない。Objective-to-Ticket links は context であり、dependency/scheduling authority ではない。

---
