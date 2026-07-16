---
title: "効果的な Memory システム設計・検証"
state: "active"
created_at: "2026-06-20T15:16:00Z"
updated_at: "2026-07-16T17:24:00Z"
linked_tickets: ["00001KSKBPHRG", "00001KT02TCCG", "00001KTGCAFXG", "00001KSKBPTHR", "00001KXMEZNYC", "00001KXMK7YMC", "00001KXNYXNM6", "00001KXMK846H"]
---

## Goal

Yoi の Memory / Knowledge / Skills / generated context / resident context / retrieval / usage metrics を、実際の開発・設計・レビュー・オーケストレーションに効く sensemaking substrate として再設計・検証する。Memory は短期・変化前提の context、Knowledge は育てる long-term note、Skill は移植可能な workflow として分け、この Objective ではそれらと authority record / docs / typed tools の境界を再整理する。

この Objective でいう「効果的な Memory システム」は、単に多く保存する仕組みではなく、作業中の問いに対して relevant material を集め、根拠を検証可能にし、再表現・仮説形成・反証探索・意思決定・成果物への反映を低コストにする仕組みである。

暫定的な定義:

- foraging cost を下げる: Ticket / Objective / current question に対して、関連する memory / docs / tickets / session evidence / code references を探しやすい。
- evidence を失わない: Memory が authority そのものにならず、Ticket / docs / git history / session logs / user instruction への検証可能な入口になる。
- schema 化を支援する: raw summary ではなく、subsystem、invariant、risk、authority boundary、open question、rejected alternative、hypothesis など推論しやすい形へ再表現できる。
- hypothesis loop を支援する: 支持証拠だけでなく、代替仮説・棄却理由・反証 evidence・stale assumption を扱える。
- product に戻る: Memory に保存して終わりではなく、Ticket、review、docs、implementation、decision、report に影響を戻せる。
- stale / contradiction を扱う: 古い前提、矛盾、適用範囲外の memory を検出・降格・更新できる。
- usage を成果基準で測る: resident exposure や read count ではなく、判断・レビュー・実装・docs に効いたかを観測できる。

## Motivation / background

現在の Memory システムは「墓場化」している。保存された情報はあるが、後続の作業で自然に使われにくく、使われたとしても根拠・適用範囲・鮮度・反証可能性が弱い。結果として Memory は、作業場ではなく古い結論の倉庫になりやすい。

Pirolli & Card 2005 の sensemaking model では、分析作業は単なる保存ではなく、次の変換として捉えられる。

```text
external data sources
-> shoebox
-> evidence file
-> schema / representation
-> hypotheses
-> presentation / product
```

Yoi の現行 Memory は、この流れのうち「保存」と「一部の検索」には対応しているが、少なくとも以下が弱い。

- Ticket / task / question ごとの shoebox がない。
- shoebox から evidence snippets を切り出し、source / provenance / applicability / confidence と共に扱う evidence file がない。
- `summary`, `decision`, `request` は durable memory storage taxonomy であり、sensemaking 用 schema としては粗い。Knowledge は古い record kind をそのまま残すのではなく、育てる long-term note subsystem として再設計する。再利用可能な手順は Skill、保守された設計資料は Knowledge / docs / Ticket decisions に寄せる。
- decision は残るが、hypothesis space、alternative、rejected reason、disconfirming evidence が残りにくい。
- reviewer / orchestrator が confirmation bias を避けるための反証探索導線が弱い。関連する手順誘導は旧 Workflow ではなく Skill と role prompt / typed tools へ寄せる。
- resident exposure と explicit retrieval は観測できても、Memory が product に効いたかは測りにくい。

この Objective は、Memory 関連の設計・検証・検討・考察を一元化し、個別 Ticket がばらばらに storage、prompt、retrieval、metrics を改善して再び墓場を増やすことを防ぐための判断背景である。

## Strategy / design direction

Memory を「長期保存領域」ではなく、Yoi の multi-agent 開発における sensemaking loop の支援機構として設計する。

### 1. Pirolli & Card の stage に合わせて責務を分ける

- external data sources: Tickets、docs、git history、session logs、reports、code、user instructions。
- shoebox: 特定 Ticket / Objective / design question に対して関連しそうな材料を集めた task-bound working set。
- evidence file: shoebox から抜き出した根拠 snippet。source anchor、支持/反証、適用範囲、confidence、staleness を持つ。
- schema / representation: subsystem、invariant、risk、authority boundary、open question、hypothesis、alternative、contradiction など、推論しやすい再表現。
- hypotheses: 採用前の設計仮説、代替案、棄却条件、反証 evidence。
- product: Ticket、review、docs、implementation、decision、report、orchestration plan などの成果物。

### 2. 最初の重点は task-bound shoebox と evidence file

Memory 墓場化の最初の原因は、保存情報が現在の問いに集まらないことである。まずは Orchestrator / Intake / Reviewer が Ticket を扱う時に、関連 memory / docs / tickets / reports / prior decisions を shoebox として束ねる導線を作る。

この段階では大きな永続 schema 追加に飛びつかず、report / Ticket artifact / bounded generated context として検証してよい。

### 3. Session Overview を extract の足場にする

現在の extract は、tool call / tool result summary を含む flat slice から意味を復元しようとして断片化しやすい。改善方針は、main Worker が通常 Assistant Message として Progress message を残し、user messages + Assistant messages を Session Overview として先に読む形にする。

- Progress message は専用 Tool ではなく通常 Message とし、ユーザーへの進捗報告と extract 用 semantic summary を兼ねる。
- extract worker には専用の read-only evidence tools を渡し、Overview で重要そうに見えた箇所だけ session range / tool summaries / source anchors を探索させる。
- extract worker は Memory / Knowledge / Skill を直接更新しない。output は必ず staging を挟み、source / provenance を host 側で機械的に保持する。
- trigger は LLM call 単位ではなく、Overview accumulation、evidence growth、Worker run cycle、task boundary を組み合わせる。mid-run 発火を入れる場合も staging/checkpoint extraction に限定する。

### 4. Memory を authority にしない

Memory は Ticket、docs、git history、session logs、user instruction の代替ではない。Memory は authority record への evidence index / schema / reasoning aid として扱う。

したがって、改善案は次の性質を持つべきである。

- source / provenance を辿れる。
- stale / superseded / contradicted を扱える。
- Memory の断定をそのまま authority として使わない。
- Ticket body/thread/artifacts を読まずに Objective や Memory だけで実装判断できる状態を作らない。

### 5. 反証探索を first-class にする

より効果的な Memory は、過去方針を思い出すだけでなく、現在案を疑うために使える必要がある。

Reviewer / Orchestrator / Intake の導線では、次を探せるようにする。

- supporting evidence
- contradicting evidence
- stale decisions
- rejected alternatives
- unresolved questions
- authority boundary risks
- prior failures / reports

### 6. Metrics は exposure から product impact へ寄せる

Memory が prompt に入った、または query されたことは成功ではない。評価は次を区別する。

- resident exposure
- explicit retrieval
- cited in response
- cited in Ticket / review / report
- changed requirement
- changed implementation
- contradicted / invalidated
- led to docs or decision update

### 7. 後続 Ticket は concrete slice に分割する

この Objective は中期的な設計・検証の一元化 record であり、umbrella Ticket ではない。実装や調査は、単独で実装・レビュー・close できる concrete Ticket に分割する。

候補 slice:

- Memory sensemaking 分析 report を `docs/report/` に作る。
- Ticket routing 用 Memory shoebox artifact を試作する。
- evidence snippet schema / source resolver を設計する。
- hypothesis / rejected alternative / disconfirming evidence の表現を追加する。
- Reviewer Skill / review process に反証探索を入れる。
- Memory usage metrics を product impact oriented に拡張する。
- stale / contradiction / renewal の検出・表示を設計する。
- turn 中の Progress message を通常 Assistant Message として残す prompt/guidance を追加する (`00001KXMEZNYC`)。
- Session Overview + Evidence index を使う extract input を設計・実装する。
- extract worker 専用の read-only evidence search/read/source-anchor tools を設計する。
- extract output を staging に限定し、source range と output entry を結びつける schema を設計する。

## Success criteria / exit conditions

- Memory システムの目的が「保存」ではなく「sensemaking loop 支援」として project records / docs / prompts / Skills で一貫して説明されている。
- Pirolli & Card の `shoebox -> evidence file -> schema -> hypotheses -> product` に対応する Yoi 内の責務と非責務が整理されている。
- Ticket / Objective / docs / session logs / Memory / Skills の authority boundary が明確で、Memory が authority を僭称せず、Skill は手順資源として外部状態 authority を持たない。
- 少なくとも一つの実作業 routing / review / design analysis で、task-bound shoebox または evidence file が生成・利用され、作業品質にどう効いたかが確認されている。
- Memory records または関連 artifacts が source / provenance / applicability / staleness / supports-or-refutes のいずれかを扱えるようになっている。
- extract が User / Assistant messages 由来の Session Overview を primary input とし、tool logs を evidence として探索できる設計になっている。
- extract worker 専用の read-only evidence tools が設計され、main Worker の tool surface を増やさない方針になっている。
- extract output は direct Memory / Knowledge / Skill write ではなく staging を挟む方針になっている。
- Reviewer / Orchestrator が supporting evidence だけでなく、contradicting evidence / stale assumptions / rejected alternatives を探す導線を持っている。
- Memory usage metrics が resident exposure と product impact を区別している。
- 古い Memory が放置されるのではなく、stale / superseded / contradicted / needs-review として扱える方針がある。
- 後続の実装 Ticket が concrete slice として分割され、Objective が Ticket dependency や進捗 container として使われていない。

この Objective は、Memory が少なくとも一つの中規模設計・実装・レビュー作業で「関連情報を見つける」「根拠を確認する」「代替案/反証を検討する」「成果物へ反映する」流れを実証し、その設計方針が docs / Skills / metrics に反映された時点で `done` を検討できる。

## Decision context

- ユーザー指摘: 「Memoryシステムが完全に墓場化している」。これは保存量不足ではなく、保存情報が現在の問い・根拠・仮説・成果物に接続されない問題として扱う。
- ユーザー指示: Memory システムの設計・検証・検討・考察を Objective にまとめ、より効果的な Memory システムを作成する目標のもとで情報を一元化する。
- 「効果的」の定義は未確定だが、当面は Pirolli & Card の sensemaking process に沿って、foraging cost、evidence quality、schema usefulness、hypothesis/disconfirmation support、product impact、staleness handling を評価軸にする。
- Memory は durable project authority ではない。Ticket、docs、git history、session logs、明示 user instruction の代替として使わない。
- Objective context は判断背景であり、個別実装の authority は各 Ticket body/thread/artifacts と明示的な Ticket relations / OrchestrationPlan records にある。
- `history` に残らない context-only injection を改善案にしない。新しい context input は history に commit する原則を守る。
- Knowledge は古い unused record kind をそのまま残すのではなく、育てる long-term Markdown note subsystem として再設計する。再利用可能な手順・作法は Agent Skills (`.yoi/skills/<skill>/SKILL.md`) へ、durable policy/rationale は Knowledge / maintained docs / Ticket decisions へ、外部状態 authority は typed feature/tool surface へ分ける。
- Generated memory / Ticket / docs / report / Skill の境界を再定義する場合は、authority boundary と migration/staleness を明示する。
- 関連する既存 Ticket:
  - `00001KSKBPHRG` — Prompt / Workflow 評価メトリクスと改善 Offer
  - `00001KT02TCCG` — Memory prompt: conditional guidance and proactive lookup
  - `00001KTGCAFXG` — Use .yoi/memory marker for repo-local memory root
  - `00001KSKBPTHR` — ワークスペースのメモリーをLintするヘッドレスCLI
  - `00001KXMEZNYC` — ターン中のProgress messageを残す指示を追加する

## Historical references / prior design sources

現在の Memory システムの初期設計時には、Codex Memories / Chronicle と HermesAgent を明示的な参考事例として調査していた。関連する調査・設計記録は、現在は主に以下に退避されている。

- `docs/.local/old-docs/ref/memory-systems.md`
- `docs/.local/old-docs/plan/memory.md`
- 初期設計 commit: `ca5a3d11` — `2026-04-21 メモリシステムの設計`
- 関連 commit:
  - `0c1276b7` — `Memoryシステムの整理・Promptカタログチケット`
  - `3d04f793` — `memoryを抽出する仕組みの実装`
  - `f1b7af62` — `docs: memoryシステムの仕様変更と、動的Tool・VCSの話`
  - `a2aecbf0` — `update: memoryシステムの"Phase"表記を撤廃`

### Codex Memories / Chronicle から得た設計要素

旧設計では、Codex Memories / Chronicle を `extract -> staging -> consolidation -> durable Markdown memory` の非同期パイプラインとして捉えていた。

主な参照点:

- extract と consolidation の 2 段構成。
- extract は JSON schema / structured output で分類ブレを抑える。
- consolidation は reasoning model / agentic rewrite によって、既存 memory と staging entries を統合・整理する。
- staging と durable memory を分ける。
- `MEMORY.md` は retrieval-oriented handbook として扱う。
- `memory_summary.md` は prompt-loaded high-signal context として扱う。
- `raw_memories.md` は routing layer / task inventory 的な中間層として扱う。
- workspace diff や usage 情報を使い、stale / deleted evidence / noisy entries を整理する。
- consolidation は append だけでなく、rewrite / merge / split / trim / drop / cleanup を担う。

Yoi 初期設計では、これを参考に以下を意図していた。

- activity token 閾値で extract を発火する。
- compact より前に session log range を抽出する。
- extract は `decisions`, `discussions`, `attempts`, `requests` などの候補を staging に保存する。
- 抽出時点では durable policy / Skill / docs へ早期分類せず、純粋な「起きたこと」に寄せる。
- consolidation が summary / decisions / requests と、必要に応じた docs / Skill / Ticket decision 更新候補を整理する。
- consolidation 入力に linter warnings / usage metrics / stale cleanup 候補を含める。
- stale / superseded / unused / noisy な情報を整理する。

この Objective での再解釈:

- Codex の `raw_memories.md` は、Pirolli & Card の sensemaking model では `shoebox` または `evidence file` に近い。
- Yoi は extract / consolidation という pipeline だけを継承しても不十分であり、task-bound shoebox / evidence file / hypothesis loop / product feedback がなければ Memory は再び墓場化する。
- 特に、staging を consolidation の一時入力としてだけ扱うと、後続 Ticket / Objective / review が使う探索入口にならない。
- Yoi では `raw memories` 相当の中間層を、現在の問いに紐づく working set / evidence index として再設計する必要がある。

### HermesAgent から得た設計要素

旧設計では、Nous Research HermesAgent を 3 層の memory system として整理していた。

- Persistent Memory:
  - `MEMORY.md` / `USER.md`
  - Markdown + SQLite / FTS5 session search
  - 起動時 system prompt snapshot
  - bounded character limits
- Skill Library:
  - procedural memory
  - `~/.hermes/skills/<name>/SKILL.md`
  - `skill_manage` tool による agentic CRUD
- User Model / Honcho:
  - dialectic user modeling
  - 外部 service 連携

HermesAgent で特に重要だった点:

- memory / skill review は一定 turn / tool iteration ごとに background agent として起動する。
- 保存すべきものがなければ `Nothing to save.` で NOP として終了する。
- Yoi extract の「空配列許容」はこの設計からも影響を受けている。
- memory は session start 時の frozen snapshot として system prompt に入り、mid-session write で prompt cache を壊さない。
- persistent memory は bounded で、limit 超過時は deterministic eviction ではなく、agent に replace / remove を促す。
- procedural memory / skills は一般 memory から分離されている。
- SQLite FTS5 + LLM summarization による cross-session recall がある。

この Objective での再解釈:

- HermesAgent の `MEMORY.md` / `USER.md` / `skills` の分離は、Yoi の Memory / Knowledge / Skills / prompt resources / docs / Ticket decision / generated memory の責務再整理に使える。
- Yoi は foreground isolation 自体をすでに持つため、取り入れるべきなのは isolation そのものではなく、Overview を足場にした maintenance / extraction の質改善である。
- main Worker が通常 Assistant Message として残す Progress message は、人間向け進捗報告と machine-readable session overview を兼ねられる。
- extract worker は専用 read-only evidence tools で必要箇所だけ探索し、direct write ではなく staging に出力する。
- reusable procedure, reviewer focus, orchestration tactic, project preference, user preference, design invariant を同じ Memory bucket に入れると墓場化しやすい。
- `Nothing to save.` / empty extraction allowed は重要だが、保存抑制だけでは効果的な Memory にはならない。保存されたものが task-bound shoebox / evidence / schema / hypothesis / product に接続される必要がある。
- frozen snapshot / prompt cache 配慮は Yoi の history/context 加工原則と整合するが、それだけでは retrieval / resurfacing / disconfirmation は解決しない。

### Lessons for the next design iteration

Codex と HermesAgent の調査から、Yoi が継承すべきものと、継承するだけでは足りないものを分ける。

継承すべきもの:

- structured extract と agentic consolidation の分離。
- staging / raw memories / durable memory の分離。
- 保存対象がなければ NOP にする発火設計。
- prompt-loaded summary と durable retrieval-oriented memory の分離。
- stale / noisy / unused entries の cleanup。
- procedural memory と declarative memory の分離。
- session search / usage metrics / linter feedback を consolidation に入れる設計。
- user / Assistant messages から作る Session Overview を semantic guide にし、tool logs を evidence として探索する設計。

足りないもの:

- Pirolli & Card の sensemaking stage における各 record の役割定義。
- Ticket / Objective / current question に紐づく task-bound shoebox。
- authority record へ戻れる evidence file / provenance / source anchor。
- hypothesis, alternative hypothesis, rejected reason, disconfirming evidence の first-class 表現。
- reviewer / orchestrator が confirmation bias を避けるための反証探索導線。
- resident exposure や read count ではなく product impact を測る metrics。
- stale / contradiction / renewal を作業中に resurfacing する導線。

したがって、次の Memory 設計は Codex / HermesAgent の単純なコピーではなく、以下を満たす必要がある。

```text
external data / sessions / tickets / docs / code
-> task-bound shoebox
-> evidence file with provenance
-> schema / representation
-> hypotheses and disconfirmation
-> Ticket / review / docs / implementation / decision product
-> product impact and stale-feedback metrics
```

この Objective では、以後の Memory 関連 Ticket / report / implementation をこの historical reference と sensemaking model の両方に照らして判断する。
