---
title: 'Panel startup latency をユーザー目線の dashboard content ready 基準で計測・改善する'
state: 'inprogress'
created_at: '2026-06-18T13:30:51Z'
updated_at: '2026-06-18T14:20:54Z'
assignee: null
readiness: 'implementation_ready'
risk_flags: ['panel', 'e2e', 'startup-latency', 'user-visible-readiness', 'dashboard-content', 'profiling']
queued_by: 'workspace-panel'
queued_at: '2026-06-18T13:55:08Z'
---

## Background

ユーザーが問題にしている `yoi panel` startup latency は、first frame や fixture の単一 Ticket row が `rows_rendered` に出るまでではなく、**ユーザーが `yoi panel` を起動してから、workspace dashboard として実際に使えるコンテンツが画面に揃って見えるまで**の時間である。

既存の `00001KV62PF32` は `panel_ready` / first frame と単一 fixture Ticket row readiness の混同を修正したが、まだ以下の点で不十分だった。

- direct subprocess spawn から単一 fixture Ticket row の `rows_rendered` までを測っているだけで、workspace dashboard 全体の content ready ではない。
- live workspace でユーザーが体感している明らかに長い遅延を再現・属性分解していない。
- fixture 上の約 120ms rows-ready をもって「追加改善不要」と判断してしまうと、ユーザー視点の問題を取り逃がす。

この Ticket では、Panel startup latency の主基準を user-visible dashboard content ready に置き直し、遅延源を計測・改善する。

## Definitions

- `panel_first_frame`: 初回 visible draw。loading / empty frame でもよい補助 metric。
- `fixture_single_row_ready`: 具体的な fixture Ticket row が `rows_rendered` に現れる補助 metric。
- `dashboard_content_ready`: ユーザーが workspace dashboard として必要な主要コンテンツが揃い、実際に画面へ描画された状態。この Ticket の主 metric。

`dashboard_content_ready` は少なくとも以下を含む。

- Ticket rows が fixture / live-like workspace の期待データと一致している。
  - id
  - title
  - state/status
  - row kind
  - primary action / disabled reason where relevant
- Pod / Companion / Orchestrator 関連 row または status が、fixture / live-like workspace の期待状態と一致している。
- orchestration overlay を含む fixture では、local / orchestration state が表示上も期待通り反映されている。
- loading / empty / partial single-row render だけでは ready とみなさない。

## Requirements

- E2E / harness の readiness event または helper を追加・修正し、`dashboard_content_ready` を測れるようにする。
  - first frame / single-row readiness とは別 metric にする。
  - event 名・test 名・log 出力から意味が誤解されないようにする。
- 測定開始点は、ユーザーの `yoi panel` 起動に十分近いものにする。
  - 基本は `Command::spawn` 直前からでよい。
  - interactive shell 入力まで含めない場合は、その範囲を test/report に明記する。
- Fixture を live-like に強化する。
  - 複数 Ticket state を含める。
  - Pod metadata / Companion / Orchestrator 表示を含める。
  - orchestration overlay を含める。
  - 必要に応じて stale socket / slow observation / many Ticket records など、実遅延の候補を再現する fixture を追加する。
- `dashboard_content_ready` は単なる `rows.len() >= N` や単一 Ticket row match だけで通さない。
  - expected dashboard snapshot / expected row set として比較する。
  - 欠落 row、wrong status、wrong action、overlay 未反映を fail にする。
- Live workspace 相当の遅延源を属性分解する。
  - Ticket scan / parsing
  - orchestration overlay worktree validation / read
  - Pod metadata scan
  - socket/status probing
  - Companion / Orchestrator lifecycle observation
  - role session / local claim scan
  - git worktree / branch checks
- 明らかに長い遅延がある場合は改善する。
  - UI 初期化を content-ready 待ちで止めないだけでは不十分。
  - 実コンテンツが揃うまでの経路自体を短くする。
  - slow source を lazy / bounded / parallel / cached / timeout-shortened にできる場合は実装する。
- Before / after の実測値を implementation report に記録する。
  - first frame
  - dashboard content ready
  - slow-source breakdown
  - fixture 条件 / live-like 条件
- 測定で改善不要と判断する場合でも、ユーザーが見ている長い live latency がなぜ再現しないか、またはどの範囲外かを明示する。

## Acceptance criteria

- E2E が `dashboard_content_ready` を主 startup latency metric として測る。
- `panel_first_frame` または単一 Ticket row readiness だけでは、この Ticket の主 E2E は通らない。
- Expected dashboard snapshot に含まれる Ticket / Pod / Companion / Orchestrator / overlay 要素が揃って描画された時点を ready として扱う。
- Missing row / wrong state / missing overlay / missing action label の fixture では ready 判定が fail する。
- User-visible dashboard content ready の before / after 実測値が記録される。
- 遅延源の breakdown が記録され、主要 slow source に対して具体的な改善または明示的な non-action rationale がある。
- Live-like fixture または current workspace に近い条件で、ユーザー体感の長い遅延を取り逃がさない。
- Existing Panel behavior に regression がない。
  - row selection
  - composer target
  - Queue action
  - orchestration overlay display
- Validation: relevant `cargo test -p yoi-e2e --features e2e panel`, `cargo check`, `cargo fmt --check`, `git diff --check`, and `nix build .#yoi` if code/package/runtime behavior changes.

## Non-goals

- Interactive shell の command lookup / prompt rendering まで含めた OS/shell latency の厳密測定。
- すべての background observation が完全 settle するまで UI を出さないこと。
- Panel architecture の全面刷新。
- Ticket lifecycle semantics の変更。

## Related work

- `00001KV62PF32` — Panel startup latency E2E を一覧データ描画完了基準に修正する。単一 fixture row readiness までで不十分だったため request-changes 済み。
- `00001KV5MRH6D` — Panel startup latency E2E / first visible frame separation work。
- `00001KV5D7MG5` — Panel orchestration worktree Ticket state overlay。
