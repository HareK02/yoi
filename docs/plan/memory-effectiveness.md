# Memory effectiveness improvement plan

## Context

`docs/plan/memory.md` で定義した永続化・検索・extract / consolidation の基盤は実装済みだが、現状の workspace memory を見る限り、実用上の効果はまだ弱い。

現在の主要な症状:

- `knowledge/*` が空で、通常 Pod が自発的に参照できる discovery 面がない
- `memory/summary.md` は「Always-on サマリ」と設計されているが、通常 Pod の system prompt へ常駐注入されていない
- consolidation は `KnowledgeCandidateReport::empty()` を受け取り、prompt 上も「候補レポートが空なら新規 Knowledge を作るな」としているため、Knowledge の cold-start が起きない
- `decisions/*` と `requests/*` は記録としては残るが、description / resident injection を持たず、後続 turn で自然に読まれにくい
- bundled prompt に INSOMNIA 開発固有の ticket / TODO 運用を前提にした shadow 禁止が入り、一般ユーザー workspace の管理文脈を過剰に落とす可能性がある

この状態で usage metrics を先に実装しても、「使われていない / 発見されない memory」を測るだけになり、Knowledge 化候補や保護閾値の信号が十分に育たない。先に memory が読まれ、Knowledge が最低限生まれる経路を作る。

## Proposal

Usage metrics の前に、memory effectiveness の最小改善として以下を行う。

1. `summary.md` を通常 Pod の resident context に入れる
2. Knowledge の cold-start gate を緩める
3. bundled memory prompts を project-management 手法に依存しない形へ汎用化する
4. 明示参照 (`#<slug>`) の実用経路を整える
5. 既存 decisions / requests から Knowledge へ昇格できる整理経路を用意する

この順序により、metrics 実装時には「実際に読まれる対象」と「昇格候補になりうる Knowledge / memory record」が存在する状態になる。

## 1. Summary resident injection

### Problem

`docs/plan/memory.md` では `memory/summary.md` を Always-on サマリとしている。しかし実装上、通常 Pod の system prompt に載るのは `model_invokation: true` の Knowledge description / Workflow description であり、summary は `MemoryRead(kind=summary)` しない限り読まれない。

これは設計と実装のズレであり、memory の中心 record が通常 turn に効かない原因になっている。

### Direction

通常 Pod の system prompt trailing section に `summary.md` の本文または圧縮表示を載せる。

- `[memory]` が有効で、`memory/summary.md` が存在する場合だけ注入する
- consolidation / internal disposable Worker には注入しない
- 既存の `Resident knowledge` とは別 section にする
- summary の frontmatter は除外し、本文のみを載せる
- summary 本文の linter 上限は 20,000 chars だが、resident 注入では別途 soft cap を持つか、当初は summary 自体を 1-5k tokens に保つ prompt 方針に依存する

### Expected effect

通常 Pod は少なくとも現在の project memory の要約を毎 turn 見られる。Knowledge が空でも memory subsystem の効果がゼロになりにくい。

## 2. Knowledge cold-start gate

### Problem

現行 consolidation は `KnowledgeCandidateReport::empty()` を渡し、prompt も空レポート時の新規 Knowledge 作成を禁止している。これは usage metrics 完成後の gate としては妥当だが、cold-start では永久に Knowledge が生まれない。

### Direction

Knowledge 作成 gate と resident injection gate を分離する。

- 新規 Knowledge 作成:
  - consolidation が high-confidence に再利用価値を判断できる場合は許可する
  - default は `model_invokation: false`
  - `description` は「本文要約」ではなく「何の知識で、いつ読むべきか」を書かせる
- `model_invokation: true` への昇格:
  - usage metrics / 人間判断 / 将来の評価指標に基づく
  - cold-start 自動作成時には原則 ON にしない
- GC / drop 保護:
  - 引き続き usage metrics の明示 invoke frequency を使う

つまり、metrics が無い間も Knowledge は蓄積できるが、system prompt 常駐コストを増やす判断は保守的にする。

### Expected effect

Knowledge が空のまま固定される問題を避ける。metrics 実装前でも、検索可能な reusable knowledge が増える。

## 3. Prompt generalization

### Problem

bundled memory prompts は、INSOMNIA 自身の ticket / TODO / worktree 運用を一般ユーザーへ押し付けている。ticket はこのプロジェクトの管理手法であり、ユーザー workspace の正本や作業管理の形は project ごとに異なる。

### Direction

Default prompt では特定の管理手法名を禁止対象として列挙しない。

残すべき一般原則:

- 既存の authoritative record を逐語的に mirror しない
- ファイル操作ログや VCS 履歴そのものを memory に再保存しない
- ただし、将来の作業判断に効く project-management 上の制約・優先順位理由・プロセス決定・ recurring pattern は抽象化して保存してよい
- INSOMNIA 自身の ticket shadow 回避は bundled default ではなく workspace / user prompt override で表現する

### Expected effect

Memory extraction / consolidation が、ユーザー workspace の管理文脈を過剰に捨てなくなる。

## 4. Explicit slug invocation path

### Problem

設計上は `#<slug>` で Knowledge を参照するが、現状は LLM が `KnowledgeQuery` / `MemoryRead` を自発的に呼ぶことに寄っている。ユーザーが明示的に `#slug` を書いても、専用の展開経路が弱い。

### Direction

段階的に実装する。

1. user input から `#<slug>` を検出する
2. exact slug で `knowledge/<slug>.md` を解決する
3. 解決できた本文を user turn の context として history に append してから LLM に渡す
4. 解決失敗は system-reminder / synthetic note ではなく、history に残る通常の input artifact として扱う

この実装では、プロジェクト指示の「history に残らない context input 禁止」に従い、展開結果を必ず `worker.history` に commit する。context だけへの差し込みはしない。

### Expected effect

ユーザーが必要な Knowledge を明示参照でき、metrics 実装後の明示 invoke 計測点にもなる。

## 5. Existing record promotion

### Problem

現状の memory には decisions と requests があるが、いくつかは reusable knowledge に近い。これらが decisions のままだと resident discovery や KnowledgeQuery の対象になりにくい。

### Direction

consolidation の tidy / consolidation に、既存 `decisions/*` / `requests/*` を Knowledge へ昇格する経路を追加する。

- 同一内容を copy するのではなく、Knowledge として再利用可能な抽象に rewrite する
- 元 decision は必要なら残す。Knowledge 側には `last_sources` として直近 source を持つ
- 新規 Knowledge 作成は §2 の cold-start gate に従う
- decision の「判断履歴」と Knowledge の「再利用可能な運用知識」を混同しない

### Expected effect

既存 memory の価値を引き出し、metrics 前に検索対象を増やせる。

## Ordering

推奨順序:

1. Prompt generalization
   - 既に `tickets/memory-prompts-remove-ticket-policy.md` として起票済み
   - prompt が不要に情報を捨てる状態を先に止める
2. Summary resident injection
   - 既存 `summary.md` が即座に通常 Pod へ効く
3. Knowledge cold-start gate
   - consolidation が Knowledge を生めるようにする
4. Existing record promotion
   - 既存 decisions / requests から Knowledge を立ち上げる
5. Explicit slug invocation path
   - Knowledge が存在する状態で明示参照経路を整える
6. Usage metrics
   - 明示 invoke / resident cost / protection threshold / Knowledge candidate report を実装する

## Interaction with usage metrics

Usage metrics は不要ではない。役割を以下に限定すると妥当性が高い。

- `model_invokation: true` へ昇格する判断材料
- resident injection cost と使われ率の観測
- tidy / GC での drop / 大幅圧縮保護
- Knowledge candidate report の補助信号

一方、metrics を Knowledge 作成そのものの唯一 gate にすると cold-start が壊れる。Knowledge 作成は high-confidence agent judgement、resident 化と保護は metrics、という分担にする。

## Risks / open questions

### Summary injection cost

`summary.md` を常駐させると system prompt budget を消費する。summary が肥大化した場合に備え、以下のどちらかを選ぶ必要がある。

- linter / consolidation prompt で summary を 1-5k tokens に保つ運用を先行する
- resident injection 側に hard truncation / warning を入れる

初期は前者でよいが、truncation されるなら LLM に明示した方がよい。

### Knowledge noise

cold-start gate を緩めると Knowledge が増えすぎる可能性がある。対策として、新規作成時の `model_invokation: false` default、update 優先、description 品質指示、tidy step の noisy 分類を使う。

### Prompt override boundary

INSOMNIA 自身の ticket shadow 回避を完全に消すと、この repository の memory は作業ログ寄りに戻る可能性がある。これは bundled default ではなく workspace prompt override で解くべきで、default prompt の責務ではない。

### `#<slug>` history representation

明示参照展開をどの Item / Segment として history に残すかは別途詰める必要がある。原則は「LLM が見たものは history に残す」。context-only injection は使わない。

## Validation criteria

この計画が妥当かは、以下で判断する。

- 新規 workspace で Knowledge が 0 件のまま固定されない
- 通常 Pod が `summary.md` の内容を tool call なしで参照できる
- bundled prompts が ticket / TODO / worktree 等の特定管理手法を前提にしない
- Knowledge の `description` が discovery 面として機能する
- `model_invokation: true` は uncontrolled に増えない
- usage metrics 実装時に、計測対象となる明示参照 / resident cost / Knowledge records が存在する
