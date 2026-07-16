---
created_at: "2026-07-15T21:33:00Z"
updated_at: "2026-07-16T03:45:00Z"
objective: "00001KVJSMQXZ"
status: "architecture-draft"
notes: "Memory / Knowledge / Skills を別々の workspace resource として再設計するための draft architecture。この文書は Objective resource であり、実装 authority ではない。"
---

# Memory / Knowledge / Skills architecture overview

## 立場

Yoi は **Memory**、**Knowledge**、**Skills** を 1 つの汎用 record store に押し込めるのではなく、別々の resource class として扱う。

sensemaking model よりも、実際に運用できる architecture の方が重要。Pirolli & Card の sensemaking process は背景知識としては有用だが、product shape が固まる前に shoebox / evidence / hypothesis infrastructure を第一級概念にする必要はない。まず目指すべきなのは、成果物が人間に読める形で成長できる clear な workspace resource model である。

目標の分割:

- **Memory**: 短期 fact、嗜好、現在の focus、進行中の context。変化する前提で書く。
- **Knowledge**: 長期的に育てる note。人間と agent が改訂し、相互リンクで mesh を形成し、durable project understanding として読めるもの。
- **Skill**: Agent Skills format に従う、移植可能で確立された手順・workflow。

これは、以前の draft にあった sensemaking artifact 中心の見方を置き換える。sensemaking は Memory / Knowledge / Skills の上に乗る usage pattern であり、中心の storage taxonomy ではない。

## Design goals

- 人間が読め、成長できる artifact を作る。
  - 一時的な model summary だけでは足りない。
  - 有用な成果は Knowledge note、Skill、Ticket decision、doc、report に育てられるべき。
- 揮発的なものと durable なものを分ける。
  - 短期 context が長期 note を汚染しないようにする。
  - 長期 note が session extraction のたびに上書きされないようにする。
- 手順と note を分ける。
  - 繰り返し使える作業方法は Knowledge note ではなく Skill にする。
- authority boundary を明示する。
  - Ticket は work authority を定義する。
  - docs と Objective resources は maintained design context を持つ。
  - Knowledge notes は育てる project understanding を持つ。
  - Skills は execution を guide する。
  - Memory は変化する working context と preferences を追跡する。
  - typed feature/tool surfaces が external state changes を所有する。
- 可能な限り Workspace backend を resource API の共有 authority にする。
  - `WorkspaceClient::Http` が使えるときに、Worker ごとに local view が分岐してはいけない。

## Resource classes

### Memory

Memory は、agent が作業を継続する助けになる volatile / short-to-medium-term な情報を扱う。長期的な project truth のふりはしない。

Memory record に入るもの:

- 現在の focus。
- user preferences。
- working assumptions。
- authority が別にある recent decisions。
- 進行中の constraints。
- あとで Ticket / doc / session を再確認するための reminder。
- session から得た observations。
- 変化することが前提の personal / workspace context。

Memory は provisional に書く:

- いつ / なぜ learned したかを書く。
- scope と applicability を書く。
- staleness / supersession を許す。
- authoritative records を verbatim にコピーしない。
- 可能なら Tickets / docs / Knowledge notes への pointer を優先する。

Memory は resident context と lightweight lookup には有用だが、permanent note system として最適化しない。

#### Memory examples

```text
User preference: prefers direct commits only when explicitly requested.
Scope: this repository / current dogfooding workflow.
Source: repeated user corrections in sessions around git operations.
Staleness: revisit if user changes repo workflow.
```

```text
Current focus: web Workspace console and Workspace-backed Ticket/Skill authority.
Source: recent Tickets and Objective updates.
Expected to change after current milestone.
```

### Knowledge

Knowledge は long-term note system。人間と agent が育て、改訂し、link し、split / merge しながら読むもの。抽出 snippet の山ではなく、project understanding の mesh を形成する。

Target Knowledge は、古い未使用の Knowledge feature をそのまま残すものではない。legacy implementation は先に削除してよい。置き換えは proper workspace note subsystem として設計する。

Knowledge notes の要件:

- Markdown-first で人間が読める。
- stable ID / slug を持つ。
- bidirectional links / backlinks を support する。
- 必要なら tags や typed relations を support する。
- 重要な claim には provenance を残す。
- Tickets、Objectives、docs、commits、reports、Skills、他 Knowledge notes に link できる。
- review / staleness / supersession を support する。
- 自動生成だけに頼らず、意図的に maintain される。

Knowledge は、長期 architecture note、conceptual model、subsystem explanation、decision context、recurring constraints、domain understanding を育てる場所である。

#### Knowledge examples

- `workspace-authority-model`
  - Workspace backend が Tickets / Skills / Runtime views の authority である理由を説明する。
  - Ticket backend API design、Skill support ticket、Workspace control plane Objective に link する。
- `memory-knowledge-skill-boundary`
  - Memory / Knowledge / Skills の boundary を定義する。
  - この architecture resource と将来の implementation Tickets に link する。
- `ticket-lifecycle-authority`
  - Ticket state authority と transition graph の rationale を説明する。
  - 関連 decisions と code locations に link する。

### Skill

Skill は、ある種類の task に対して確立された、portable な workflow / procedure。Agent Skills format に従う。

```text
.yoi/skills/<skill-name>/
  SKILL.md
  scripts/
  references/
  assets/
```

Skill は state machine ではなく、external authority も所有しない。agent が available tools を使って task をどう実行するかを示す prompt/resource guidance である。

Skill に含めるもの:

- いつその Skill を使うか。
- step-by-step procedure。
- expected inputs。
- expected outputs / report shape。
- examples。
- edge cases。
- optional references / scripts / assets。

Skill は、project-specific assumptions が少なく、別 workspace に移しても使えるとき portable と言える。

#### Skill examples

- `coder-review-cycle`
  - Coder が実装、検証、review request、feedback 対応、dossier 作成をどう行うか。
- `ticket-intake`
  - 曖昧な user request を accepted Ticket requirements に変換する方法。
- `architecture-review`
  - design proposals、alternatives、authority boundaries を評価する方法。

## Boundaries

### Memory vs Knowledge

Memory は provisional / change-oriented。Knowledge は maintained / growth-oriented。

Memory を使うべきとき:

- 情報が短命。
- preference や working assumption である。
- resident context として有用。
- 長期的な置き場所がまだ明確でない。

Knowledge を使うべきとき:

- 時間をかけて読み直し、改訂するべき情報。
- durable project concept を説明する情報。
- 複数の future tasks から link されるべき情報。
- backlinks / mesh structure が有用な note。
- 人間が project understanding として browse できるべきもの。

Promotion path:

```text
Memory observation -> candidate note/update -> Knowledge note / docs / Ticket decision
```

Promotion は明示的に行う。すべての Memory item が Knowledge になるわけではない。

### Knowledge vs Docs

Docs は public / project-facing な maintained exposition。Knowledge は internal で、link され、発展する understanding。

Knowledge note は後で doc になり得るが、threshold は違う:

- Knowledge は uncertainty、partial models、evidence links を含められる。
- Docs は settled explanations または user/developer guidance を提示するべき。

### Knowledge vs Ticket decisions

Ticket decisions は work item history と state の authority。Knowledge notes は複数 Ticket をまたいだ synthesis。

ある decision が Ticket の requirement、state、acceptance criteria を変えるなら、それは Ticket に記録する。Knowledge はそこに link し、より広い pattern を説明できる。

### Skill vs Knowledge

Knowledge は「何が true か」「project をどう理解するか」を説明する。Skill は「 recurring task をどう実行するか」を説明する。

Skill を使うべきとき:

- review process。
- implementation process。
- release checklist。
- architecture evaluation method。

Knowledge を使うべきとき:

- Workspace authority model。
- Memory architecture。
- Ticket lifecycle rationale。

### Skill vs Feature/Plugin

Skill は prompt/resource guidance。Feature/Plugin は executable authority と tool surface。

Skill は「何をどう進めるか」を書けるが、Ticket、Workspace、Memory、外部状態を変更する権限そのものは持たない。その権限は Feature/Plugin や typed tool/API が持つ。

例: Skill は「review 前に Ticket shoebox を作る」と指示できる。実際に作成する authority は Workspace / Memory feature の typed tool/API が提供する。

## Workspace authority

Target architecture は Workspace-backed にする。

### Memory API

Workspace backend が最終的に提供するもの:

- Memory list / search / read / write / edit / delete。
- resident memory summary の生成または取得。
- staleness / supersession markers。
- sessions や artifacts からの Memory candidate proposal。
- provenance と audit events。
- preference / current-focus surfaces。

移行期間中は local `.yoi/memory` を compatibility / offline storage として残してよい。

### Knowledge API

Workspace backend は、raw filesystem layout を唯一の interface にするのではなく、proper note API を提供する。

- Knowledge catalog / list / search。
- note read / write / edit / delete。
- link / backlink extraction。
- relation / tag metadata。
- staleness / supersession markers。
- note diagnostics / lint。
- source / provenance refs。
- Markdown files からの import / export。

Filesystem representation は残してよい。おそらく `.yoi/knowledge/` 配下になる。ただし Worker / Runtime / Web / CLI は、利用可能なら Workspace API view に収束する。

### Skill API

Skill support は separate Skill Ticket の方針に従う。

- Workspace backend が discovery / lint / catalog / activation を所有する。
- `.yoi/skills/<skill>/SKILL.md` は workspace storage convention。
- `WorkspaceClient::Http` が使えるとき、Workers は Workspace API から Skill metadata / body を使う。
- Skill references / assets は backend-resolved authority または Skill resource APIs 経由で access する。

## Human-readable growth path

中核要件は、有用な material が人間に読める形へ成長できること。

典型的な path:

```text
Session observation
  -> Memory record
  -> Knowledge note candidate
  -> Knowledge note with links/backlinks
  -> maintained doc or Ticket decision if it becomes authority
```

```text
Repeated successful procedure
  -> Memory observation / Ticket comment
  -> draft Skill
  -> `.yoi/skills/<skill>/SKILL.md`
  -> builtin or shared Skill if portable
```

```text
Design discussion
  -> Objective resource
  -> Knowledge note synthesis
  -> implementation Tickets
  -> docs after stabilization
```

Architecture は、これらの promotion を explicit and reviewable にする。

## Sensemaking as a usage pattern

Sensemaking は有用だが、resource model の上に重なる pattern として扱う。

Pirolli & Card の flow は Yoi resource にこう mapping できる:

```text
External sources -> Memory/search/shoebox artifact -> Knowledge notes / Objective resources -> Tickets/docs/Skills/products
```

ただし、実際の workflows が必要性を示すまでは、`shoebox` や `hypothesis matrix` を mandatory first-class concepts にしない。

初期の sensemaking support は lightweight でよい:

- Ticket / Objective artifacts としての task-bound collected references。
- provenance 付き evidence summaries。
- review Skills における explicit contradictory evidence sections。
- recurring patterns を synthesize する Knowledge notes。

## Extract / review mechanism comparison

Yoi は foreground 応答生成からの隔離をすでに持っている。次に改善すべきなのは isolation そのものではなく、extract worker が意味のない flat slice から文脈を復元しようとしている点である。

Target は、通常 Assistant Message を session overview の backbone とし、extract worker が専用の read-only evidence tools で必要箇所だけ探索し、出力は必ず staging を挟む形にする。

```text
Main Worker
  -> user-facing Progress / final Assistant messages を history に残す
  -> tool calls / tool results は evidence log として残る
  -> overview が一定量溜まる、または run/task boundary に到達する
  -> extract worker が Overview を読む
  -> extract worker が専用 evidence tools で必要範囲だけ探索する
  -> write_extracted で structured staging payload を出す
  -> consolidation / review が Memory / Knowledge / Skill candidate を扱う
```

### Current Yoi pipeline

現在の Yoi extraction は activity-log pipeline:

- `build_extract_input` が conversation slice を flat Markdown として render する。
  - user / assistant text は保持する。
  - tool-call names を含める。
  - raw tool-result content ではなく tool-result summaries のみを含める。
  - reasoning は落とす。
- extract worker の tool は `write_extracted` 1 つだけ。
- `write_extracted` は `decisions`、`discussions`、`attempts`、`requests` を持つ structured `ExtractedPayload` を 1 件受け取る。
- LLM は provenance を作らない。Worker が `StagingRecord` を書くときに `source` を機械的に付与する。
- empty payload は valid で、no-op として扱える。
- consolidation は後で staging entries、full current Memory records、usage evidence、tidy hints を consume する。
- consolidation は Memory tools 経由で write し、linter feedback に対応し、record を merge / replace し、outdated / superseded / unused / noisy records を clean up する。

この価値は、provenance と audit boundary が clear なこと。特に Knowledge notes、design records、Tickets、docs、その他 durable project understanding に影響し得る material に向いている。

弱点は、flat slice だけでは「なぜその tool を使ったのか」「何のための探索だったのか」「どの時点の結論なのか」が抜けやすいこと。extract worker が local な tool summary から意味を推測すると、断片的な attempts や無用な discussions が増えやすい。

### HermesAgent / Codex からの再解釈

HermesAgent は background review により、conversation snapshot から Memory / Skill update を直接判断する。参考になるのは、maintenance を foreground interaction から隔離し、保存すべきものがなければ NOP にする点である。一方で、direct write-only review は Yoi には強すぎる。staged evidence layer がないと、auditability を失いやすく、one session への overfit や過剰 rewrite が起きやすい。

Codex は session / rollout を durable source として扱い、background pipeline が後から claim / extract / consolidate する。参考になるのは、turn-local sidecar そのものではなく、durable source、phase separation、claim/lease/retry/global lock、workspace diff/baseline guard の発想である。

Yoi では両者を単純コピーせず、次のように分ける:

- foreground isolation は既存設計の前提として扱う。
- main Worker は、人間にも有用な Progress message を通常 Assistant Message として残す。
- extract worker は、Overview を semantic guide として読み、必要な evidence だけを read-only tool で探索する。
- extract worker は durable Memory / Knowledge / Skill を直接書かず、必ず staging に structured output を出す。
- consolidation / review が、staging を Memory update、Knowledge candidate、Skill candidate、Ticket/doc update candidate に整理する。

### Overview-first extract

Overview は、committed history にある user messages と normal Assistant messages から作る。Assistant messages には、最終応答だけでなく、長い作業中の Progress message も含める。

Progress message は専用 Tool ではなく通常 Assistant Message とする。Tool surface を増やさず、ユーザーへの進捗報告と extract 用 semantic summary を兼ねるためである。内容は public に見せられる短い作業状態に限る。

Overview に含めるもの:

- user requests / corrections / approvals;
- Assistant progress messages;
- Assistant final responses;
- parent delegation や Ticket context のように、history に commit された task context;
- tool evidence への bounded index。

Overview に含めないもの:

- raw reasoning / chain-of-thought;
- raw tool-result content 全文;
- secret-like data;
- history に commit されていない hidden context injection。

### Extract worker evidence tools

extract worker には専用の constrained evidence tools を渡す。これは main Worker の tool 数を増やすものではなく、extract 専用 worker の read-only surface である。

必要な tool category:

- **Evidence search**: session slice / tool summaries / message index から query で候補 range を探す。
- **Evidence read**: bounded な session entry range、tool call/result summary、必要なら host が許す bounded content excerpt を読む。
- **Source anchor resolver**: `session_id`, entry range, tool call id, file path, Ticket/Objective artifact ref などを staging source に結びつける。
- **write_extracted**: structured payload を 1 回提出する既存 output tool。

制約:

- read-only。file write、Ticket mutation、Memory/Knowledge/Skill direct write は持たせない。
- bounded output。large tool result は summary / excerpt / pointer に留める。
- provenance first。extract worker が根拠を推測せず、読んだ evidence range を output と結びつけられるようにする。
- NOP allowed。Overview を読んで保存価値がなければ empty payload で終わる。

### Staging remains mandatory

extract output は必ず staging を挟む。

理由:

- Progress message は semantic guide であり、authority ではない。
- extract worker の判断は候補であり、direct Memory / Knowledge / Skill update にしない。
- staging があることで source range、review、consolidation、stale cleanup、later synthesis を維持できる。
- Knowledge / Skill / docs のような long-term artifacts は、staging と review / consolidation を経て育てるべき。

### Trigger policy

発火単位は LLM call 単位にしない。LLM call 単位では文脈が薄く、断片的な extraction になりやすい。

初期方針:

- overview token count または Assistant Progress/final message count が一定量を超えたら extract を予約する。
- tool evidence growth が大きい場合も extract を予約できる。
- Worker run cycle / task boundary / Ticket phase boundary では extract を試す。
- long-running 中に mid-run 発火する場合も、direct update ではなく staging/checkpoint extraction に限定する。

この trigger は「意味のある overview が溜まったか」と「evidence が失われる前に staging したいか」の両方で判断する。

### Target Yoi lanes

Yoi は 1 つの generic extractor ではなく、maintenance lanes に分ける。

1. **Overview-guided extract lane**
   - user / Assistant messages から Overview を作る。
   - extract worker が dedicated evidence tools で必要箇所だけ探索する。
   - output は必ず staging。
   - decisions / discussions / attempts / requests に加え、将来の Memory / Knowledge / Skill candidate の材料になる source anchors を残す。

2. **Consolidation / tidy lane**
   - staging entries、existing Memory、usage evidence、linter feedback、stale/noisy hints を統合する。
   - legacy Knowledge as generated memory 前提を外し、Knowledge は redesigned note subsystem への candidate として扱う。

3. **Memory update lane**
   - short-term facts、preferences、current focus、stale Memory cleanup を扱う。
   - direct write/edit/delete を許す場合でも、extract worker ではなく別 lane の bounded Memory API と audit event で扱う。

4. **Skill / Knowledge candidate lane**
   - Skill は recurrence / portability rules で gate する。
   - Knowledge は linked Markdown notes への proposal / candidate とし、自動 direct rewrite を避ける。

この lane split により、session は人間にも機械にも追いやすくなり、extract は断片的な tool log ではなく Overview を足場にして evidence を確認できる。Yoi の auditability は staging によって維持する。

## Implementation posture

現在の実装は redesign してよい。既存の Memory / Knowledge shape があるからという理由で残さない。

ただし big-bang rewrite は避ける。この architecture が accepted されてから分割する。

Recommended implementation sequence:

1. **Knowledge removal 後の current Memory を clarify する**
   - short-term / resident Memory を維持する。
   - Memory が Knowledge を代替しなければならない、という前提を外す。
   - legacy `knowledge/*` を generated memory として扱う stale consolidation prompt language を削除する。

2. **Progress message guidance を追加する**
   - 長い作業や tool loop の節目で、main Worker が通常 Assistant Message として短い Progress message を残す。
   - 専用 Tool は追加しない。
   - Progress message は public に見せられる作業状態、確認済み事実、判断、未解決点、次の作業に限定する。

3. **Overview-first extract input を設計する**
   - user messages + Assistant messages を semantic Overview として優先する。
   - tool calls / tool results は Evidence index として分離する。
   - committed history だけから Overview を作り、hidden context injection を避ける。

4. **Extract worker 専用 evidence tools を設計する**
   - extract worker に read-only Evidence search / Evidence read / Source anchor resolver を渡す。
   - main Worker の tool surface は増やさない。
   - output tool は `write_extracted` を維持し、structured payload を 1 回提出させる。
   - source range と output entry を結びつけられる schema / staging format を設計する。

5. **Staging-first extract を実装する**
   - extract worker は direct Memory / Knowledge / Skill write をしない。
   - overview accumulation / evidence growth / run or task boundary で extract を予約する。
   - mid-run 発火を入れる場合も staging/checkpoint extraction に限定する。

6. **Target Knowledge note model を設計する**
   - Markdown note format。
   - link / backlink model。
   - provenance / staleness metadata。
   - Workspace API surface。
   - reviewable Knowledge updates の candidate / staging format。

7. **Consolidation / tidy lane を更新する**
   - staging entries、existing Memory、usage evidence、linter feedback、stale/noisy hints を統合する。
   - legacy Knowledge as generated memory 前提を外す。
   - Memory update、Knowledge candidate、Skill candidate を分けて扱う。

8. **Minimal Knowledge catalog/read/write を実装する**
   - Markdown files と Workspace API から始める。
   - lint と backlinks を追加する。

9. **Skill support を別に実装する**
   - Agent Skills standard と Workspace authority に従う。
   - Skill と Knowledge note schema を混ぜない。
   - automatic Skill modification は recurrence と portability で gate する。

10. **Promotion workflows/tools を追加する**
    - Memory -> Knowledge candidate。
    - Knowledge -> docs / Ticket decision candidate。
    - repeated procedure -> Skill candidate。

11. **Resource classes が安定してから sensemaking helpers を追加する**
    - collected refs。
    - evidence extraction。
    - contradiction / staleness views。
    - product-impact metrics。

## Non-goals

- Memory を唯一の long-term knowledge store として扱うこと。
- Knowledge を名前だけ変えた generated memory として扱うこと。
- Skill を Workflow tracker や state machine として扱うこと。
- Worker history / tool results の外で context injection を隠すこと。
- Knowledge notes を Tickets / docs / git history より authoritative にすること。
- review なしに Knowledge / Skills / docs を自動 rewrite すること。
- human-readable artifact model が安定する前に vector database を設計すること。

## Open decisions

- Target Knowledge は `.yoi/knowledge/<slug>.md`、nested directories、index + notes のどれを使うか。
- Knowledge notes の exact frontmatter。
- Link syntax と backlink extraction rules。
- Knowledge note IDs / slugs と titles の関係。
- Memory は local-first のままにするか、Knowledge と同じ phase で Workspace API-first にするか。
- Memory / Knowledge APIs は同じ crate にするか、別 domain crates にするか。
- Memory -> Knowledge の promotion UI/tool をどうするか。
- personal Memory と workspace Memory をどう区別するか。
- Knowledge drafts の auto-generation をどこまで許すか。
- extract worker 専用 evidence tools の exact API: search/read/resolver の引数、上限、source anchor format。
- Overview trigger を overview token count、Assistant message count、evidence growth、run/task boundary のどれで制御するか。
- mid-run extract をどこまで許すか。初期は staging/checkpoint extraction に限定する方針。
- source range と `ExtractedPayload` entries をどう結びつけるか。
- どの Memory sidecar writes を direct に許し、どれを staged proposals にするか。extract worker 自体は direct write しない。
- Skill sidecar output は patches/proposals だけから始めるか、low-risk Skill edits を auto-apply してよいか。
- Memory / Knowledge / Skill Workspace APIs をまたぐ sidecar audit events をどう表現するか。
- prompt-cache-aware sidecar input で full replay と digest-plus-tail をどう選ぶか。

## Exit criteria for architecture phase

この architecture は、次が満たされたら Tickets に分割できる。

- Memory / Knowledge / Skill boundary が accepted される。
- long-term linked notes としての target Knowledge が accepted される。
- Memory / Knowledge / Skills の Workspace backend authority が accepted される。
- Overview-first extract、extract worker 専用 evidence tools、staging-first output の方針が accepted される。
- 最初の implementation slice が選ばれる。
- non-goals が accepted され、old Workflow tracking や old unused Knowledge をそのまま再作成しないことが確認される。
