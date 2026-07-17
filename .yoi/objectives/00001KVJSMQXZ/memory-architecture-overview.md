---
created_at: "2026-07-15T21:33:00Z"
updated_at: "2026-07-16T18:20:00Z"
objective: "00001KVJSMQXZ"
status: "architecture-draft"
notes: "Memory / Knowledge / Skills を別々の workspace resource として再設計するための draft architecture。この文書は Objective resource であり、実装 authority ではない。"
---

# Memory / Knowledge / Skills architecture overview

## 1. Position

Yoi は **Memory**、**Knowledge**、**Skills** を 1 つの汎用 record store に押し込めず、別々の resource class として扱う。

最初に固めるべきなのは、成果物が人間に読める形で成長できる workspace resource model である。Pirolli & Card の sensemaking process は有用な背景知識だが、storage taxonomy を shoebox / evidence / hypothesis として先に固定しない。sensemaking は Memory / Knowledge / Skills の上に乗る usage pattern として扱う。

Target split:

- **Memory**: 短期 fact、嗜好、現在の focus、進行中の context。変化する前提で書く。
- **Knowledge**: 長期的に育てる note。人間と agent が改訂し、相互リンクで mesh を形成し、durable project understanding として読めるもの。
- **Skill**: Agent Skills format に従う、移植可能で確立された手順・workflow。

この文書の中核は次の 2 つである。

1. resource class の境界を明確にする。
2. session から extract / staging / consolidation を経て resource に至る pipeline の責務を明確にする。

## 2. Design goals

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

## 3. Resource model

### 3.1 Memory

Memory は、agent が作業を継続する助けになる volatile / short-to-medium-term な情報を扱う。長期的な project truth のふりはしない。

Memory record に入るもの:

- 現在の focus。
- user preferences。
- working assumptions。
- authority が別にある recent decisions の要約や pointer。
- 進行中の constraints。
- あとで Ticket / doc / session を再確認するための reminder。
- session から得た observations。
- 変化することが前提の personal / workspace context。

Memory は provisional に書く:

- いつ / なぜ learned したかを書く。
- どの前提で learned したかを必要な範囲で書く。
- staleness / supersession を許す。
- authoritative records を verbatim にコピーしない。
- 可能なら Tickets / docs / Knowledge notes への pointer を優先する。

Memory は resident context と lightweight lookup には有用だが、permanent note system として最適化しない。

#### 3.1.1 Memory storage profile: bounded H2 Markdown file

Memory の初期 storage profile は、bounded な single Markdown file にする。

Memory は Knowledge のような note bundle ではない。1〜3 行程度の short items を H2 section ごとに並べる resident context surface として扱う。

初期 filesystem shape:

```text
.yoi/memory/
  memory.md
  _staging/
  _resolutions.jsonl
```

`memory.md` は H2 section を基本単位にする。

```md
# Workspace Memory

## Current focus

- Memory extract redesign is focused on Overview-first extraction and staging resolution.
  Source: Objective 00001KVJSMQXZ. Stale when related tickets close.

## Preferences

- User prefers implementation Tickets, not design-only Tickets.
  Source: 2026-07-16 session.

## Working assumptions

- Knowledge should be OKF-compatible, while volatile Memory and staging should not be OKF.
  Source: architecture objective.

## Reminders

- Re-check extract/consolidation prompts after staging resolution is implemented.
```

Recommended H2 sections:

- `## Current focus`
- `## Preferences`
- `## Working assumptions`
- `## Constraints`
- `## Reminders`
- `## Stale or superseded`

Each item should stay short. When useful, include `Source` and `Stale when` inline. Long explanations, evidence-heavy analysis, durable rationale, citations, and cross-linked concepts should be routed to Knowledge rather than expanded inside Memory.

The single-file layout is an initial storage profile, not an API contract. Workers, Web, Runtime, and CLI should use the Workspace Memory API view rather than depending on the exact file layout, so storage can later split or evolve without changing the model-visible contract.

#### Memory examples

```text
User preference: prefers direct commits only when explicitly requested.
Source: repeated user corrections in sessions around git operations.
Staleness: revisit if user changes repo workflow.
```

```text
Current focus: web Workspace console and Workspace-backed Ticket/Skill authority.
Source: recent Tickets and Objective updates.
Expected to change after current milestone.
```

### 3.2 Knowledge

Knowledge は long-term note system。人間と agent が育て、改訂し、link し、split / merge しながら読むもの。抽出 snippet の山ではなく、project understanding の mesh を形成する。

Target Knowledge は、古い未使用の Knowledge feature をそのまま残すものではない。legacy implementation は先に削除してよい。置き換えは proper workspace note subsystem として設計する。

Knowledge notes の要件:

- Markdown-first で人間が読める。
- OKF-compatible な concept document として扱える。
- stable path / slug を持ち、OKF concept ID として参照できる。
- Yoi 内部で move / rename に強い identity が必要な場合は、extension frontmatter として `yoi_id` を持てる。
- bidirectional links / backlinks を support する。
- Obsidian-style wiki links (`[[slug]]`, `[[slug|label]]`) を support し、Knowledge mesh の authoring shorthand として使える。
- 必要なら tags や typed relations を support する。
- 重要な claim には provenance / citations を残す。
- Tickets、Objectives、docs、commits、reports、Skills、他 Knowledge notes に link できる。
- review / staleness / supersession を support する。
- 自動生成だけに頼らず、意図的に maintain される。

Knowledge は、長期 architecture note、conceptual model、subsystem explanation、decision context、recurring constraints、domain understanding を育てる場所である。

#### 3.2.1 Knowledge format profile: OKF-compatible bundle

Yoi Knowledge は、可能な限り Open Knowledge Format (OKF) compatible な bundle として設計する。

OKF から採用する baseline:

- Knowledge bundle は Markdown file tree とする。
- non-reserved `.md` file は concept document とする。
- concept document は YAML frontmatter + Markdown body とする。
- path without `.md` を OKF concept ID として扱う。
- `type` は required field とする。
- `title`, `description`, `resource`, `tags`, `timestamp` は recommended field とする。
- normal Markdown links を OKF-compatible graph edges として扱う。
- Obsidian-style wiki links (`[[slug]]`, `[[slug|label]]`) も graph edges として扱い、Markdown links へ解決・export できるようにする。
- `index.md` は progressive disclosure のための directory listing として使える。
- `log.md` は agent-readable update history として使える。
- `# Citations` section は external source / authority reference を示す convention として使う。
- consumers は unknown frontmatter fields、unknown `type`、broken links を tolerant に扱う。

Yoi は OKF に extension frontmatter を足してよい。候補:

```yaml
yoi_id: 00001...
status: draft | active | stale | superseded
source_refs: []
authority_refs: []
objective_refs: []
ticket_refs: []
skill_refs: []
reviewed_at: 2026-07-16T00:00:00Z
staleness: "Revisit when ..."
supersedes: []
superseded_by: null
```

Filesystem shape の例:

```text
.yoi/knowledge/
  index.md
  log.md
  architecture/
    index.md
    memory-architecture.md
    workspace-authority.md
  references/
    pirolli-card-2005-sensemaking.md
```

OKF compatibility は Knowledge の exchange / storage profile であり、Memory / staging / Skill の format ではない。

- Memory は短期・変化前提の resident context store なので OKF にしない。
- staging は extract / consolidation の審査キューなので OKF にしない。
- Skill は Agent Skills format を維持する。OKF `type: Playbook` に吸収しない。

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

### 3.3 Skill

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

## 4. Resource boundaries and authority

### 4.1 Memory vs Knowledge

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

### 4.2 Knowledge vs Docs

Docs は public / project-facing な maintained exposition。Knowledge は internal で、link され、発展する understanding。

Knowledge note は後で doc になり得るが、threshold は違う:

- Knowledge は uncertainty、partial models、evidence links を含められる。
- Docs は settled explanations または user/developer guidance を提示するべき。

### 4.3 Knowledge vs Ticket decisions

Ticket decisions は work item history と state の authority。Knowledge notes は複数 Ticket をまたいだ synthesis。

ある decision が Ticket の requirement、state、acceptance criteria を変えるなら、それは Ticket に記録する。Knowledge はそこに link し、より広い pattern を説明できる。

### 4.4 Skill vs Knowledge

Knowledge は「何が true か」「project をどう理解するか」を説明する。Skill は「recurring task をどう実行するか」を説明する。

Skill を使うべきとき:

- review process。
- implementation process。
- release checklist。
- architecture evaluation method。

Knowledge を使うべきとき:

- Workspace authority model。
- Memory architecture。
- Ticket lifecycle rationale。

### 4.5 Skill vs Feature/Plugin

Skill は prompt/resource guidance。Feature/Plugin は executable authority と tool surface。

Skill は「何をどう進めるか」を書けるが、Ticket、Workspace、Memory、外部状態を変更する権限そのものは持たない。その権限は Feature/Plugin や typed tool/API が持つ。

例: Skill は「review 前に Ticket shoebox を作る」と指示できる。実際に作成する authority は Workspace / Memory feature の typed tool/API が提供する。

## 5. Workspace API authority

Target architecture は Workspace-backed にする。

### 5.1 Memory API

Workspace backend が最終的に提供するもの:

- Memory list / search / read / write / edit / delete。
- resident memory summary の生成または取得。
- staleness / supersession markers。
- sessions や artifacts からの Memory candidate proposal。
- provenance と audit events。
- preference / current-focus surfaces。

移行期間中は local `.yoi/memory` を compatibility / offline storage として残してよい。初期 storage profile は H2 section ベースの bounded `.yoi/memory/memory.md` だが、これは API contract ではない。

### 5.2 Knowledge API

Workspace backend は、raw filesystem layout を唯一の interface にするのではなく、proper note API を提供する。

- Knowledge catalog / list / search。
- note read / write / edit / delete。
- OKF-compatible frontmatter validation / normalization。
- link / backlink extraction from Markdown links and wiki links (`[[slug]]`)。
- relation / tag metadata。
- staleness / supersession markers。
- note diagnostics / lint。
- source / provenance refs and citations。
- Markdown files からの import / export。
- OKF bundle import / export profile。

Filesystem representation は `.yoi/knowledge/` 配下の OKF-compatible bundle を第一候補にする。ただし Worker / Runtime / Web / CLI は、利用可能なら raw filesystem ではなく Workspace API view に収束する。

### 5.3 Skill API

Skill support は separate Skill Ticket の方針に従う。

- Workspace backend が discovery / lint / catalog / activation を所有する。
- `.yoi/skills/<skill>/SKILL.md` は workspace storage convention。
- `WorkspaceClient::Http` が使えるとき、Workers は Workspace API から Skill metadata / body を使う。
- Skill references / assets は backend-resolved authority または Skill resource APIs 経由で access する。

## 6. Session-to-resource pipeline

この章が extract 改善の中心である。`extract -> staging -> consolidate` の分割は維持する。3 つは同じ memory maintenance pipeline の一部だが、判断の種類が違う。

```text
Session history
  -> Overview + Evidence index
  -> extract
  -> staging
  -> consolidation
  -> Memory / Knowledge candidate / Skill candidate / Ticket-doc candidate / discard
```

役割の要約:

- **extract**: session から候補を拾う。recall 寄り。Memory 化はしない。
- **staging**: provenance 付き候補キュー。まだ Memory ではない。
- **consolidation**: staging を審査・剪定・統合する。precision 寄り。Memory 肥大化と陳腐化を防ぐ。

### 6.1 Overview: transcript as semantic backbone

Overview は、committed history にある user messages と normal Assistant text outputs から作る。Assistant text outputs には、最終応答だけでなく、長い作業中の Progress message も含める。

Progress message は専用 Tool ではなく、ordinary user-visible prose response として残す。Tool surface を増やさず、ユーザーへの進捗報告と extract 用 semantic summary を兼ねるためである。

Overview に含めるもの:

- user requests / corrections / approvals。
- Assistant progress messages。
- Assistant final responses。
- parent delegation や Ticket context のように、history に commit された task context。
- tool evidence への bounded index。

Overview に含めないもの:

- raw reasoning / chain-of-thought。
- raw tool-result content 全文。
- secret-like data。
- history に commit されていない hidden context injection。

Overview の意図は、extract worker に「何のための探索だったか」「どの判断が節目だったか」「どこが未解決か」を伝えることである。Overview は authority ではない。durable output には evidence と source anchors が必要である。

### 6.2 Extract: candidate generation

Extract は candidate generation であり、Memory 化ではない。runtime 側の extract worker が Overview と session evidence を読んで、後で記憶化を検討すべき **flat candidate records** を staging に出す。

Extract の責務:

- Overview を読んで、記憶化を検討すべき candidate を見つける。
- 必要な箇所だけ evidence tools で確認する。
- candidate ごとに bounded evidence snippets / source anchors を選ぶ。
- candidate ごとに `stage_candidate` を呼び、1 candidate = 1 staging record として保存する。
- 最後に `finish_extraction` を呼び、staged count または NOP reason を残す。

Extract がしてはいけないこと:

- Memory / Knowledge / Skill / Ticket / docs を直接変更しない。
- batch payload として複数 candidate を 1 staging record にまとめない。
- tool result 全文や reasoning を無制限に取り込まない。
- local slice だけから根拠を推測しない。
- tool call chronology、generic progress、current focus update を抽出しない。
- 「進捗報告があった」こと自体を Memory 化しない。

Staging candidate は、Consolidation がそれ単体で discard / merge / promote / defer を判断できる最小単位にする。

旧 `decisions` / `discussions` / `attempts` / `requests` batch schema は維持しなくてよい。互換性よりも、flat candidate records と clear responsibility を優先する。

#### 6.2.1 Extract candidate taxonomy

Extract が抽出する対象は activity log ではない。抽出対象は次の candidate kinds に絞る。

| kind | extract で見る観点 | consolidation での扱い |
| --- | --- | --- |
| `preference` | ユーザーまたは workspace の継続的な好み・作法。単発指示ではなく、今後の agent behavior に効くもの。 | Memory に短く merge / replace する候補。既存 preference と重複するなら統合。Ticket/docs の要件そのものなら mirror せず authority link へ。曖昧なら discard / defer。 |
| `working_assumption` | 現時点で仮に置いている設計・実装前提。future work に影響し、変更条件や反証条件があり得るもの。 | Memory に入れる場合は short-lived assumption として stale condition 必須。長期設計なら Knowledge candidate。すでに authority record に反映済みなら Memory には pointer だけ、または discard。 |
| `constraint` | 今後守るべき境界・禁止・invariant。実装や review でチェック可能なもの。 | active implementation に効くなら Memory。durable policy なら Ticket decision / Knowledge / docs candidate。破られた既存 Memory があれば mark_stale / replace。 |
| `decision` | alternatives / chosen / rationale がある判断。単なる事実確認や作業進行ではないもの。 | まず authority routing を判断する。Ticket 要件・state・acceptance に関係するなら Ticket decision/comment candidate。長期設計なら Knowledge/Objective candidate。短期実装判断だけ Memory に短く置く。会話中の一時結論なら discard。 |
| `open_question` | 未解決で後続作業に影響する問い。next action が書けるもの。単なる会話中の疑問ではない。 | Memory reminder にするか、Ticket follow-up / planning item に送る。解決済みなら discard。長く残る概念的問いなら Knowledge candidate。TTL / defer reason を持たせる。 |
| `lesson` | 検証・失敗・試行から得た再利用価値のある学び。同じ失敗を避ける、作業方法を改善する、Skill 化できる可能性があるもの。 | Memory に短く残すか、recurring / portable なら Skill candidate。単なる tool execution result は discard。validation evidence として Ticket/report に残すべきものは authority link。 |

抽出しないもの:

- `current_focus` update。これは resident summary surface であり、extract candidate kind ではない。
- tool call chronology。
- file read/write history。
- generic progress updates。
- one-off chit-chat。
- resolved local confusion。
- assistant self-corrections without durable consequence。
- authoritative Ticket/docs/git facts copied verbatim。
- validation results unless they imply a reusable lesson, active blocker, or authority evidence。
- implementation details that belong only in commit diff。

### 6.3 Extract worker tools

extract worker には `session-explore` feature の constrained tools だけを渡す。これは main Worker の tool 数を増やすものではない。

Tool surface:

- `search_evidence`: session snapshot / tool summaries / message index から query で候補 range を探す。
- `read_evidence`: bounded な session entry range、tool call/result summary、host が許す bounded excerpt を読む。
- `stage_candidate`: 1 candidate を staging record として保存する。
- `finish_extraction`: extract run を終了し、staged count または NOP reason を記録する。

`stage_candidate` は direct Memory write ではない。これは staging write だけを行う output tool である。

`stage_candidate` input の概念形:

```json
{
  "kind": "decision",
  "claim": "Overview is runtime-only extract projection, not staging data.",
  "why_useful": "Clarifies extract/consolidate responsibility boundary.",
  "staleness": "Revisit if Workspace can directly explore runtime sessions.",
  "evidence_ids": ["E001", "E002"]
}
```

Host wrapper は `evidence_ids` を runtime reference environment から解決し、bounded evidence snippets と source anchors を staging record に機械的に付与する。LLM に自由な source anchor を書かせない。

`finish_extraction` の概念形:

```json
{
  "staged_count": 0,
  "reason": "Only local progress and tool chronology; no durable candidates."
}
```

候補が無い場合は `stage_candidate` を呼ばず、`finish_extraction` で NOP を明示する。tool call なし終了も host 側では NOP fallback として扱ってよいが、基本は `finish_extraction` を要求する。

制約:

- read-only evidence access。file write、Ticket mutation、Memory/Knowledge/Skill direct write は持たせない。
- bounded output。large tool result は summary / excerpt / pointer に留める。
- provenance first。extract worker が根拠を推測せず、読んだ evidence ids を `stage_candidate` に渡す。

### 6.4 Staging: flat provenance-backed candidate records

Staging は Memory ではない。Staging は、extract が切り出した candidate を provenance 付きで保管し、consolidation が後で審査できるようにする queue である。

Staging は flat records にする。

```text
1 extract run = 0..N staging records
1 staging record = 1 candidate = 1 consolidation decision unit
```

1 record の概念形:

```json
{
  "schema_version": 2,
  "id": "stg_...",
  "extract_run_id": "er_...",
  "source": {
    "segment_id": "segment-1",
    "range": [120, 180]
  },
  "kind": "constraint",
  "claim": "Extract worker must not direct-write Memory/Knowledge/Skill; it only writes staging.",
  "why_useful": "Preserves runtime/embedded responsibility boundary.",
  "staleness": "Revisit if extract and consolidation move into the same Workspace execution context.",
  "evidence": [
    {
      "id": "E001",
      "kind": "message",
      "entry_range": [132, 133],
      "excerpt": "extract worker は Memory / Knowledge / Skill を直接更新しない",
      "summary": "User and architecture discussion fixed staging-only extract boundary."
    }
  ],
  "source_refs": [
    {
      "evidence_id": "E001",
      "evidence_kind": "message",
      "entry_range": [132, 133]
    }
  ]
}
```

Staging の責務:

- candidate kind / claim / hints を保持する。
- extract が選んだ bounded evidence snippets を保持する。
- source anchors を保持する。
- extract と consolidation を decouple する。
- duplicate / defer / discard / consumed の追跡対象になる。
- crash / cancel / long-running work の途中でも、後から審査できる候補を残す。

Staging がしてはいけないこと:

- Memory として resident context に直接入らない。
- Knowledge note の代替にならない。
- raw session log や Overview 全体の保存場所にならない。
- extract run 単位の batch file として複数 candidate を抱え込まない。
- staging entry をすべて Memory 化する前提にしない。

Pirolli & Card 的には、staging は shoebox / evidence file に近い。ただし Yoi の storage taxonomy そのものを shoebox にするのではなく、審査前の evidence-backed candidate queue として扱う。

### 6.5 Staging resolution: the important filter

Staging から Memory 化する段階が、Memory 肥大化と陳腐化を防ぐ中核 filter である。

Consolidation は staging entry ごとに resolution / disposition を決めるべきである。

Disposition action 候補:

- `discard`: 保存価値なし。discard reason を残す。
- `merge_memory`: 既存 Memory に統合する。
- `replace_memory`: 古い Memory を新しい内容で置換する。
- `mark_memory_stale`: 既存 Memory が古くなったことを記録する。
- `delete_memory`: 邪魔または誤った Memory を削除する。
- `defer`: 価値判断できないため短期保留する。TTL / defer reason を持つ。
- `promote_to_knowledge_candidate`: long-term note に育てる候補へ送る。
- `promote_to_skill_candidate`: recurring / portable procedure の候補へ送る。
- `link_to_authority`: Ticket / doc / commit / Objective への pointer だけ残す。
- `create_ticket_or_doc_candidate`: authority / public guidance にすべき候補として送る。

Resolution に残すべき情報:

- staging entry id。
- action。
- reason。
- source anchors。
- target record / candidate destination。
- consolidation run id / consumed_by。
- reviewed_at。
- discard / defer / stale reason。

Consumed staging を削除する場合も、resolution は残す。これにより「何を Memory にしなかったか」が改善材料として残る。

### 6.6 Consolidation: memoryization, routing, and gardening

Consolidation は precision-oriented pass である。staging を読んで、Memory にするか、別 resource candidate に送るか、捨てるかを決める。

Consolidation の責務:

- staging entry を candidate kind ごとの扱いに沿って評価する。
- Memory 化するなら usefulness / staleness / source を要求する。
- 既存 Memory と merge / replace / mark stale / delete する。
- decision / constraint / lesson などを authority / Knowledge / Skill / Ticket / docs candidate に routing する。
- discard reason / defer reason を残す。
- linter feedback / usage evidence / tidy hints を使う。
- Memory bloat と stale records を防ぐ。

Consolidation がしてはいけないこと:

- staging entry を無条件に append しない。
- Ticket / docs / git / session log の mirror を Memory に作らない。
- Knowledge / Skill / docs を background review だけで無制限に rewrite しない。
- source / provenance のない claim を durable Memory にしない。

Memory 化に必要な minimum fields / prose:

```text
content: 何を覚えるか
why_useful: 今後なぜ役立つか
source: staging / evidence anchor
staleness: 何が起きたら古くなるか
destination_reason: なぜ Knowledge / Skill / Ticket / docs ではなく Memory なのか
```

Consolidation は append worker ではなく garden worker である。新規作成より、既存 record の統合・置換・陳腐化・削除を優先する。

### 6.7 Trigger policy

発火単位は LLM call 単位にしない。LLM call 単位では文脈が薄く、断片的な extraction になりやすい。

初期方針:

- 現行通り、Worker run cycle が完了してから threshold を判定し、超えていれば extract を発火する。
- extract 開始時点で immutable snapshot / Overview projection / Evidence index を作る。
- LLM call ごとには発火しない。
- Run 中の Overview accumulation trigger / mid-run extract は初期実装に含めない。
- 将来 long-running 中に mid-run 発火を入れる場合も、direct update ではなく staging/checkpoint extraction に限定する。

この trigger は、まず既存の post-run memory job model を保ち、extract の入力品質と staging record 粒度の改善に集中する。

## 7. Sensemaking interpretation

Sensemaking は storage taxonomy ではなく、pipeline の見方として使う。

Pirolli & Card の flow を Yoi に対応させるとこうなる:

```text
External data sources
  = session log, tool results, Tickets, docs, code, web refs

Shoebox
  = Overview + Evidence index + selected candidate ranges

Evidence file
  = source anchors 付き staging entries

Schemas / hypotheses
  = Memory 化すべきか、Knowledge/Skill に送るべきか、stale かの判断

Product
  = Memory update, Knowledge candidate, Skill candidate,
    Ticket/doc update, review evidence, discard resolution
```

重要なのは、Memory 化を evidence から product へ進める審査の一形態として扱うこと。Memory record は「将来の作業に効く」という hypothesis なので、why_useful、staleness、source が必要である。

初期 sensemaking support は lightweight でよい:

- task-bound collected references as Ticket/Objective artifacts。
- provenance 付き evidence summaries。
- review Skills における explicit contradictory evidence sections。
- recurring patterns を synthesize する Knowledge notes。
- staging resolution に discard / defer / promote reason を残す。

## 8. Human-readable growth paths

中核要件は、有用な material が人間に読める形へ成長できること。

Typical paths:

```text
Session observation
  -> staging candidate
  -> Memory record
  -> Knowledge note candidate
  -> Knowledge note with links/backlinks
  -> maintained doc or Ticket decision if it becomes authority
```

```text
Repeated successful procedure
  -> staging candidate / Memory observation / Ticket comment
  -> Skill candidate
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

## 9. External reference lessons

### 9.1 Current Yoi pipeline

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

残す価値がある点:

- foreground 応答生成から memory maintenance が隔離されている。
- provenance が host 側で機械付与される。
- extract が direct write せず staging を挟む。
- empty / no-op が許される。
- consolidation が linter feedback と tidy hints を使う。

変えるべき点:

- flat slice ではなく Overview-first / Evidence-index-second にする。
- extract worker に read-only evidence tools を持たせる。
- staging を一時バッファではなく審査キューとして扱う。
- consolidation に entry-level disposition / resolution を要求する。
- legacy Knowledge as generated memory 前提を外す。

### 9.2 HermesAgent

HermesAgent は background review により、conversation snapshot から Memory / Skill update を直接判断する。

参考になる点:

- maintenance を foreground interaction から隔離する。
- 保存すべきものがなければ NOP にする。
- Memory と Skill を分ける。
- prompt snapshot / drift / injection guard を重視する。

そのまま採用しない点:

- direct write-only review を主経路にしない。
- aggressive Skill update bias を避ける。
- `MEMORY.md` / `USER.md` two-file model を Yoi 全体の architecture にしない。

Yoi では、Hermes 的な direct maintenance は bounded Memory update lane では参考になるが、extract worker 自体は direct write しない。

### 9.3 Codex

Codex は session / rollout を durable source として扱い、background pipeline が後から claim / extract / consolidate する。

参考になる点:

- durable source。
- phase separation。
- claim / lease / retry / global lock。
- workspace diff / baseline guard。
- extraction と consolidation の分離。

Yoi では、Codex 的な durable job / phase pipeline は staging resolution と consolidation scheduling に取り入れる価値がある。ただし、この architecture phase ではまず Overview-first extract と staging resolution を優先する。

## 10. Implementation posture

現在の実装は redesign してよい。既存の Memory / Knowledge shape があるからという理由で残さない。

ただし big-bang rewrite は避ける。この architecture が accepted されてから分割する。

Recommended implementation sequence:

1. **Knowledge removal 後の current Memory を clarify する**
   - short-term / resident Memory を維持する。
   - Memory が Knowledge を代替しなければならない、という前提を外す。
   - legacy `knowledge/*` を generated memory として扱う stale consolidation prompt language を削除する。

2. **Progress message guidance を追加する**
   - 長い作業や tool loop の節目で、main Worker が ordinary user-visible prose response として短い Progress message を残す。
   - 専用 Tool は追加しない。
   - Progress message は public に見せられる作業状態、確認済み事実、判断、未解決点、次の作業に限定する。

3. **Overview-first extract input を実装する**
   - user messages + Assistant text outputs を semantic Overview として優先する。
   - tool calls / tool results は Evidence index として分離する。
   - committed history だけから Overview を作り、hidden context injection を避ける。

4. **Extract worker 専用 evidence tools と staging output tools を実装する**
   - extract worker に read-only Evidence search / Evidence read を渡す。
   - main Worker の tool surface は増やさない。
   - output tools は `stage_candidate` / `finish_extraction` にする。
   - `stage_candidate` は 1 candidate = 1 flat staging record を書く。
   - source range と candidate record を結びつけられる schema / staging format を実装する。

5. **Staging-first extract を維持する**
   - extract worker は direct Memory / Knowledge / Skill write をしない。
   - overview accumulation / evidence growth / run or task boundary で extract を予約する。
   - mid-run 発火を入れる場合も staging/checkpoint extraction に限定する。

6. **Staging resolution / disposition を実装する**
   - staging entry ごとに discard / merge / replace / stale / delete / defer / promote / link を記録する。
   - consumed staging を削除する前に resolution log / archive を残す。
   - discard reason と defer reason を extract prompt 改善に使えるようにする。

7. **Target Knowledge note model を設計する**
   - OKF-compatible Markdown concept document / bundle profile。
   - Yoi extension frontmatter。
   - link / backlink model。
   - provenance / citations / staleness metadata。
   - Workspace API surface。
   - reviewable Knowledge updates の candidate / staging format。

8. **Consolidation / tidy lane を更新する**
   - staging entries、existing Memory、usage evidence、linter feedback、stale/noisy hints を統合する。
   - legacy Knowledge as generated memory 前提を外す。
   - Memory update、Knowledge candidate、Skill candidate を分けて扱う。

9. **Minimal Knowledge catalog/read/write を実装する**
   - OKF-compatible Markdown files と Workspace API から始める。
   - frontmatter validation / lint と backlinks を追加する。
   - `index.md` progressive disclosure と `# Citations` の扱いを実装する。

10. **Skill support を別に実装する**
    - Agent Skills standard と Workspace authority に従う。
    - Skill と Knowledge note schema を混ぜない。
    - automatic Skill modification は recurrence と portability で gate する。

11. **Promotion workflows/tools を追加する**
    - Memory -> Knowledge candidate。
    - Knowledge -> docs / Ticket decision candidate。
    - repeated procedure -> Skill candidate。

12. **Resource classes が安定してから sensemaking helpers を追加する**
    - collected refs。
    - evidence extraction。
    - contradiction / staleness views。
    - product-impact metrics。

## 11. Non-goals

- Memory を唯一の long-term knowledge store として扱うこと。
- Knowledge を名前だけ変えた generated memory として扱うこと。
- Skill を Workflow tracker や state machine として扱うこと。
- Worker history / tool results の外で context injection を隠すこと。
- Knowledge notes を Tickets / docs / git history より authoritative にすること。
- review なしに Knowledge / Skills / docs を自動 rewrite すること。
- extract worker に direct resource mutation authority を持たせること。
- extract run 単位の batch staging record に複数 candidate を抱え込ませること。
- staging entry をすべて Memory 化すること。
- human-readable artifact model が安定する前に vector database を設計すること。
- volatile Memory や staging queue を OKF concept document として扱うこと。
- H2 Markdown single-file layout を Workspace Memory API の長期 contract として固定すること。
- Agent Skills format を OKF Playbook documents で置き換えること。

## 12. Open decisions

- Knowledge bundle の exact directory organization: flat files、nested directories、domain directories のどれを初期推奨にするか。
- Yoi extension frontmatter の exact fields。
- Yoi stable `yoi_id` を必須にするか optional にするか。
- OKF `type` values の初期 convention をどうするか。
- Link syntax は OKF-compatible Markdown links と Obsidian-style wiki links (`[[slug]]`, `[[slug|label]]`) を support する。canonical internal representation と export normalization をどうするか。
- Knowledge note IDs / slugs と titles の関係。
- Memory は local-first のままにするか、Knowledge と同じ phase で Workspace API-first にするか。
- Memory / Knowledge APIs は同じ crate にするか、別 domain crates にするか。
- Memory -> Knowledge の promotion UI/tool をどうするか。
- personal Memory と workspace Memory をどう区別するか。
- Knowledge drafts の auto-generation をどこまで許すか。
- extract worker 専用 evidence tools の exact API: search/read の引数、上限、evidence id format。
- `stage_candidate` / `finish_extraction` の exact tool schema。
- flat staging record の exact fields: `claim`, `why_useful`, `staleness`, `evidence`, `source_refs`, `extract_run_id` など。
- Overview trigger を overview token count、Assistant message count、evidence growth、run/task boundary のどれで制御するか。
- mid-run extract をどこまで許すか。初期は staging/checkpoint extraction に限定する方針。
- staging resolution log / archive の保持期間と compact policy。
- どの Memory sidecar writes を direct に許し、どれを staged proposals にするか。extract worker 自体は direct write しない。
- Skill sidecar output は patches/proposals だけから始めるか、low-risk Skill edits を auto-apply してよいか。
- Memory / Knowledge / Skill Workspace APIs をまたぐ sidecar audit events をどう表現するか。
- prompt-cache-aware sidecar input で full replay と digest-plus-tail をどう選ぶか。

## 13. Exit criteria for architecture phase

この architecture は、次が満たされたら Tickets に分割できる。

- Memory / Knowledge / Skill boundary が accepted される。
- OKF-compatible bundle / Markdown concept documents としての target Knowledge が accepted される。
- Memory / Knowledge / Skills の Workspace backend authority が accepted される。
- Overview-first extract、extract worker 専用 evidence tools、staging-first output の方針が accepted される。
- staging resolution / disposition と、staging -> Memory 化 filter の方針が accepted される。
- 最初の implementation slice が選ばれる。
- non-goals が accepted され、old Workflow tracking や old unused Knowledge をそのまま再作成しないことが確認される。
