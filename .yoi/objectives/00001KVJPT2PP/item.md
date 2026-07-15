---
title: "Team workspace control plane and runtime architecture"
state: "active"
created_at: "2026-06-20T14:26:29Z"
updated_at: "2026-07-15T21:18:00Z"
linked_tickets: ["00001KVMFFYVX", "00001KWMBAA6V"]
---

## Goal

Yoi を、単一のローカル開発ディレクトリで動くエージェント実行ツールから、チームで作業・判断・実行結果を管理できるワークスペース基盤へ発展させる。

この Objective の中心は、Web から扱える管理システムを作り、その管理システムにローカル Runtime・リモート Runtime・将来のクラウド Runtime を接続できるようにすることである。管理システムは Ticket、Objective、Memory、Skill catalog、Artifact、Policy、Actor、Repository、Runtime state の正本を持つ。Runtime はその管理システムから Worker launch request / config bundle / repository target / authority を受け取り、作業環境を用意して Worker を実行し、結果・イベント・証跡を返す。

この Objective は Git ホスティングサービスを作るものではない。Git は重要な Repository provider として扱うが、Yoi の Workspace は Git Repository root と同じものにしない。Yoi が作るべきものは、コード・ドキュメント・データ・成果物などの Repository と Runtime を接続しながら、人間とエージェントの作業、Ticket lifecycle、Memory、Skill catalog、検証証跡、実行環境配置を管理するチームワークスペースである。

## Glossary

この Objective では、以下の語をこの意味で使う。

- Workspace: チームまたはプロジェクトの管理単位。Ticket、Objective、Memory、Skill catalog、Artifact、Policy、Actor、Repository、Runtime state を持つ。Git Repository root ではない。
- Control plane: Workspace の正本を持ち、Web UI / API / CLI から操作される管理システム。
- Runtime: Worker 群を束ねる実行基盤。Worker lifecycle、sandbox、mount、cache、checkout/worktree/container filesystem などの working directory materialization、event/control plane を管理する。将来的には 1 つの Runtime が複数 Workspace / Repository の Worker を抱えられる。
- Worker: Runtime が管理する 1 つの agent/session/process。Runtime が用意した working directory と authority の中で動く。
- Repository: Workspace に接続される source/storage。コード、ドキュメント、local directory、object storage、artifact store、dataset などを含む。Git Repository も Repository の一種であり、基本的には filesystem path ではなく URI / URL で識別する。
- RepositoryId: Workspace 内でどの Repository を対象にするかを指す安定 identifier。Git hash、branch、path ではない。
- Repository provider: Repository の種類ごとの実装。Git、local filesystem、object store、artifact store、将来の non-Git VCS など。
- RepositorySelector: Repository provider に渡す未解決の地点指定。branch/tag/PR/revspec/bookmark/revset/path@revision/object version/latest など provider-specific な symbolic / mutable / query-like locator であり、それ自体は再現性の authority ではない。
- RepositoryPoint: RepositorySelector をある時点で解決した具体地点。Git commit/tree、Mercurial changeset、SVN revision、object store version/manifest digest、file snapshot など provider ごとの immutable / reproducible point を表し、Artifact/evidence に残す。
- working directory: Runtime が Worker のために作る作業環境。1 つ以上の RepositoryPoint から materialize される作業用ディレクトリ、container filesystem、sandbox mount の集合であり、Git worktree、clone、sparse checkout などはこれを作る手段である。Browser-facing UI/API では product `Workspace` と混同しないよう、この呼称に寄せる。`Volume` は storage backing の候補名に留め、作業領域そのものの呼称にはしない。
- Ticket: チームで扱う作業単位。目的、要件、判断、議論、完了条件、関係、証跡を持つ。
- Objective: 複数の Ticket を束ねる長期目標や設計方針。
- Artifact: Ticket や Worker 実行に紐づく成果物や証跡。diff、log、validation result、review result、report など。
- Memory: エージェントやユーザーが再利用するための要約された文脈。Ticket や Artifact の正本ではない。
- Skill catalog: `.yoi/skills` / builtin skills から Workspace backend が解決する procedural guidance catalog。外部状態 authority は持たず、Ticket / Worker / workdir などの操作は typed feature/tool surface が担う。
- Actor: 人間、エージェント、システム、外部サービスなど、Workspace 上で操作や発言を行う主体。

## Motivation / background

現在の Yoi は、ローカルの `.yoi` ディレクトリ、ローカルプロセス、Ticket ファイル、ワークツリー運用によって、自分自身の開発に使えるエージェント実行環境になっている。しかし、チーム利用、Web UI、リモート実行、クラウド実行、最終的な SaaS 提供を考えると、次の前提を変える必要がある。

- Workspace を Git Repository root と同一視しない。
- ローカル filesystem 上の `.yoi` を、長期的なチーム用正本 store にしない。
- Ticket をローカル作業メモではなく、チームの作業調整 record にする。
- 実行証跡は Ticket thread、Artifact、WorkerRef snapshot、Runtime event として扱い、独立した実行単位概念を先に増やさない。
- 管理システムと Runtime を分ける。
- まず Web から Ticket、Objective、Memory、Skill catalog、Artifact、Runtime / Worker state を見られるようにする。
- 最初はローカル Runtime を使い、後でリモート Runtime、クラウド Runtime、runtime pool、resource allocation、quota、billing、sandboxing に拡張する。
- Git ホスティング機能を取り込むのではなく、Git Repository / worktree / clone は Repository provider と working directory materialization の手段として扱う。

OSS として Control plane、Runtime、Web frontend、protocol を公開しつつ、managed service では hosted control plane、runtime fleet、リソース柔軟性、team auth、backup、audit、availability、multi-tenant operations で価値を出す。

## Strategy / design direction

### 1. Control plane を先に作る

Team Workspace の正本は server-side control plane に置く。`.yoi` は local backend、single-user/self-hosted compatibility、offline/export/import、local projection、migration bridge として残せるが、multi-user SaaS の正本とはみなさない。

Control plane は Ticket、Objective、Memory、Skill catalog、Artifact、Actor、Permission、Audit、Repository、Runtime / Worker state を管理する。Web UI、CLI、TUI、将来の desktop client は、この Control plane を操作する client であり、別の正本 store を持たない。

### 2. Workspace と Repository を同一視しない

Workspace はチームまたはプロジェクトの作業管理単位である。Repository は Workspace に接続される source/storage である。Git Repository は Repository の一種にすぎない。

1 つの Workspace は複数の Repository を持てる。Repository は filesystem path ではなく URI / URL で識別する。例として `git+https://...`、`file://...`、`s3://...`、`artifact://...`、将来の VCS provider URI などを扱えるようにする。

Ticket と Objective は Repository 配下に置かず、Workspace 配下に平たく持つ。Ticket は必要に応じて対象 RepositoryId、RepositorySelector、path scope、必要 capability を持つ。Objective は複数 Ticket にまたがる target default / scope hint を持てるが、Repository の所有物にはしない。

RepositorySelector は Git branch/tag の抽象化ではない。Selector は provider-specific な未解決 locator であり、Git provider なら branch/tag/ref/revspec/PR/commit、Mercurial provider なら bookmark/revset/changeset、SVN provider なら path/revision、object store provider なら prefix/version/latest などを解釈する。実行時には Control plane または Repository provider が Selector を RepositoryPoint に解決し、Runtime はその RepositoryPoint を materialize する。

Worker launch request は Ticket の target selector を concrete RepositoryPoint に解決し、その RepositoryPoint から Runtime が working directory を materialize する。Git worktree 相当の機能は、この working directory を作るための実装戦略として扱う。

Backend は cwd や `--workspace` を暗黙の Repository として扱わない。`--workspace` は当面 workspace config root / local descriptor root を指すだけであり、Repository registry は明示設定された Workspace config から構築する。短期的には `.yoi/workspace-backend.local.toml` の `[[repositories]]` を local descriptor として使い、`uri = "."` のような local repository も明示 entry として登録する。`./` を暗黙 Repository として自動採用しない。

`.yoi` は現在の local backend / fs-store / compatibility surface として残るが、long-term Backend store ではない。将来的には `~/.yoi` 側に Backend store と Workspace registry を置き、1 Backend process が複数 Workspace を扱える形へ移行する。`.yoi/workspace-backend.local.toml` はその移行までの workspace-local descriptor / override surface として扱う。

短期的には Git を主な Repository provider とする。ただし Yoi の authority model を Git object、Git branch、Git Repository root、worktree path に固定しない。Orchestration は Git そのものではなく、`resolve_ref`、`materialize`、`diff`、`patch`、`commit`、`merge` などの Repository capability に依存する。

### 3. Ticket を team coordination record にする

Ticket は実行そのものではない。Ticket は「何を、なぜ、どの条件で完了とみなすか」を持つ。Ticket は Workspace に平たく所属し、Repository には所属しない。コードやドキュメントを対象にする Ticket は、対象 Repository / ref selector / path / intent を target として持つ。

Ticket target は intent/selector であり、実行再現性のための immutable point ではない。Worker launch request が target selector を concrete RepositoryPoint に解決し、Runtime が実際にどの revision/snapshot を materialize したかを Artifact / evidence として記録する。

```text
Ticket
  -> target selectors: Repository + ref selector + path + intent
  -> resolved RepositoryPoint
  -> working directory
  -> WorkerRef / Artifact / Evidence
  -> Review / Decision
  -> Audit / Notification
```

Target 例:

```text
Ticket targets:
  - repository: main-code
    role: primary
    ref: develop
    paths: ["crates/pod/"]
    intent: change
  - repository: docs
    role: related
    ref: main
    paths: ["docs/development/"]
    intent: read

Worker launch materialization:
  - repository: main-code
    requested_ref: develop
    resolved_point: git commit abc123
    mount: /workspace/main-code
```

Ticket には次の概念が必要になる。

- Actor identity: human / agent / system / service account.
- Assignment / owner / reviewer / watcher.
- Typed thread events: comment, decision, plan, review, implementation report, state transition.
- Linked Objective / Artifact / WorkerRef / Repository / RepositoryPoint / working directory.
- Permission / visibility.
- Audit trail.
- Notification / mention.
- Board / queue / planning / review / done / archived views.
- Conflict handling and concurrent editing policy.

### 4. Memory / Skill catalog の本格再設計は後回しにする

Memory は Ticket / Artifact のコピーではない。再利用可能な文脈、方針、学習された制約を扱うが、Ticket や Artifact の authority を置き換えない。Skill catalog は procedural guidance の catalog であり、外部状態 authority を持たない。Knowledge record kind は削除方針なので、この Objective では separate Knowledge storage を新しい control plane entity として増やさない。

理由は、Memory と Skill catalog の正しい設計が Workspace control plane の record model、Actor / visibility / permission、Ticket、Artifact / evidence、RepositoryPoint、Runtime に渡す context の監査方法に依存するためである。これらが固まる前に Memory schema や Skill API だけを作ると、local `.yoi` 前提や現行 agent runtime 前提に引っ張られ、後で再設計が必要になる。

この Objective では、Memory / Skill catalog について以下の platform contract だけを維持する。

- Memory は Control plane が扱う record だが、Ticket / Artifact の authority を置き換えない。
- Skill catalog は Workspace backend が扱う prompt/resource catalog だが、Ticket / Worker / workdir / queue の authority を持たない。
- 将来、Memory と Skill catalog の canonical storage / API は Workspace control plane 側に置く。
- local `.yoi` memory と `.yoi/skills` は compatibility、offline/export/import、local projection、migration bridge として扱う。
- Personal Memory、Workspace Memory、Worker Summary、Skill catalog は分離が必要である。
- Generated Memory には provenance、visibility、approval、audit が必要である。
- Runtime / Worker に渡した Memory / Skill context は、将来 ContextPack などとして Artifact/evidence に記録できる必要がある。

本格的な Memory 再設計は、Memory の保存先を Workspace backend / control plane record に移すタイミングで回収する。それまでは低リスクな観察、問題例の収集、既存 local memory の互換維持に留める。

### 5. Control plane / Runtime を分離する

Control plane は正本と調整を持つ。Runtime は Worker 群と実行基盤を管理する。初期実装では local backend と runtime process が同じマシン上にあり、役割が近く見えるが、設計上は分ける。

初期形:

```text
Web UI / Control Plane
  -> Runtime registry / local backend
  -> Runtime process
  -> Workers
  -> Existing Yoi tools, working copy, build/test commands
```

この段階では、現在ローカル管理画面が行っている Ticket 選択、エージェント起動、レビュー起動、作業用 checkout 作成、検証実行、結果表示を、Web/control plane から local Runtime に対して実行できるようにする。

長期的には Runtime が作業環境の用意まで請け負う。Runtime は Worker launch request / config bundle / repository target / authority を受け取り、必要な checkout、worktree、container filesystem、sandbox mount、cache、secret boundary を準備して Worker を起動する。Runtime 起動時に特定 Workspace path を必須にする形は暫定であり、Workspace / Repository 情報は Runtime process 起動引数ではなく Worker launch request 側の入力に寄せる。

Sandbox と authority 分離が成立している前提では、1 つの Runtime が複数 Workspace / Repository の Worker を抱えられる。したがって Runtime identity は Git repository root や single workspace directory と同一視しない。Runtime は execution substrate、Workspace は作業管理 record の正本として扱う。

Worker の作業環境を用意する経路は次のようにまとめる。

```text
Ticket / user intent
  -> RepositoryId + RepositorySelector + path scope + required authority
  -> resolved RepositoryPoint
  -> WorkerLaunchRequest / ConfigBundle / AuthorityBundle
  -> Runtime WorkingDirectoryMaterializer
  -> working directory allocation
  -> Worker process
  -> WorkerRef + Artifact/evidence
```

Control plane は Workspace authority、Ticket target、Repository registry、Actor permission、設定 bundle の正本を持つ。Control plane または Repository provider は RepositorySelector を RepositoryPoint に解決し、どの地点を対象にしたかを evidence に残す。Runtime は RepositoryPoint、materialization policy、sandbox policy、mount/cache/secret policy、Worker config を受け取り、Runtime-local な working directory を確保して Worker を起動する。

この境界では、Worker は host filesystem path や Repository credential を自分で発見しない。Worker は Runtime が materialize した working directory root、mount、環境変数、tool authority、config bundle だけを見る。Runtime は working directory の lifecycle、cleanup、cache reuse、namespace、quota、sandbox boundary、event collection を管理する。Control plane / Browser-facing API は raw host path、secret、socket、internal runtime path を authority-bearing internals として扱い、必要な evidence だけを Artifact として公開する。

v0 materializer は existing local root を明示的な working directory として返してよい。ただし型と呼び出し順序は、後で Git worktree、clone、sparse checkout、container filesystem、remote object snapshot、multi-repository mount に置き換えられる形にする。Runtime process 起動時の `--workspace` はこの v0 materializer の legacy bootstrap input であり、Runtime identity や long-term workspace binding ではない。

その後で、remote Runtime、self-hosted Runtime、hosted cloud runtime fleet、runtime pool、resource allocation、quota、billing、sandbox、network policy、secret distribution を追加する。

```text
Phase 1: Web control plane + local Runtime
Phase 2: Remote/self-hosted Runtime
Phase 3: Hosted cloud runtime fleet
Phase 4: Resource allocation / scheduling / quotas / billing / isolation
```

### 6. Web frontend を先に作る

Desktop app は対応コストが高いので、まず Web frontend を primary UI とする。

- Web: チームで使う主要 UI。
- CLI: automation、scripting、local operations。
- TUI/local panel: fallback、dogfooding surface。
- Future desktop: Web/control-plane model が安定した後に検討する optional client。

Web UI は Ticket、Objective、Memory、Skill catalog、Runtime、Worker、Artifact を扱う。UI の都合で正本を二重化しない。

### 7. 多重起動コストと runtime placement を見直す

Cloud/remote execution を成立させるには、多数のエージェント実行を安く管理できる必要がある。logical Worker session と Runtime process/resource placement を分ける。Runtime は Git Repository root や Workspace path に固定されず、request ごとに必要な working directory を materialize できる実行基盤として扱う。

初期 Workspace DB では、Worker を canonical table として永続化しない。Runtime / Worker 一覧は backend-local runtime inspection や将来の Runtime protocol から逐次取得する live view とし、Ticket に関わった Worker は Ticket thread events と WorkerRef snapshot / TicketWorkerLink として記録する。

Worker の一元管理、データ永続化、アーカイブは将来的には必要になる。これは Runtime protocol、remote/self-hosted/hosted runtime lifecycle、worker identity、retention policy、audit requirements が固まった後に、dedicated Worker registry / archive model として追加する。v0 で Pod metadata の代替として Worker table を作らない。

検討対象:

- Worker identity と Runtime process/resource placement の分離。
- 1 Runtime が複数 Workspace / Repository の Worker を抱える場合の namespace、quota、cleanup、audit boundary。
- Runtime-side working directory materialization、sandbox、mount、checkout/worktree/container filesystem の責務。
- Provider client、tool registry、resource cache の共有可能性。
- Prompt/resource/profile/config bundle resolution cache。
- Model call multiplexing and scheduling。
- Tool execution sandbox reuse。
- Plugin instance / Service runtime との統合。
- Session/event stream と runtime lifecycle の分離。
- Runtime-local cache、checkout reuse、build cache、dependency cache。

## Initial phases / candidate tickets

1. **Vocabulary / architecture record**
   - Workspace / RepositoryId / RepositorySelector / RepositoryPoint / working directory / Runtime / Worker / Control Plane / Ticket / Memory の用語と境界を固める。
2. **Team-space canonical data model**
   - Ticket / Objective / Target / Artifact / Actor / Permission / Audit / Memory の entity/event model を設計する。
3. **Ticket evidence model**
   - Ticket lifecycle、WorkerRef、Artifact、validation evidence、review evidence、Ticket thread の責務を明確化する。
4. **Memory storage migration boundary**
   - Memory の本格再設計は後回しにし、まずは Workspace backend に移す時の platform contract、compatibility/cache/export 方針、将来の provenance / visibility / approval 要件だけを固定する。
5. **Control plane backend architecture**
   - local `.yoi` backend と server-side canonical backend の境界、migration/export/import、compatibility mode を設計する。
6. **Web control plane MVP design**
   - read-only Ticket / Objective / Memory / Runtime / Worker state UI/API の範囲を決める。
7. **Local Runtime protocol design**
   - Web/control plane から local Runtime に安全な操作を送り、Runtime が Worker lifecycle と working directory materialization を担う protocol と authority boundary を設計する。
8. **Repository and working directory materialization model**
   - Repository URI、Repository provider capability、RepositorySelector resolution、RepositoryPoint evidence、Git worktree / clone / sparse checkout / future source backend を Runtime-side materialization strategy として抽象化する。
9. **Remote/hosted runtime foundation**
   - runtime registration, heartbeat, capability advertisement, job assignment, logs/events, secrets, sandbox/resource policy を設計する。

## Non-goals

- Git hosting service を作ること。
- `.yoi` filesystem をそのまま SaaS canonical store にすること。
- 最初から full hosted cloud execution を作ること。
- local execution / CLI / TUI / local panel を捨てること。
- Ticket を単なる issue tracker clone にすること。
- Memory を Ticket/Artifact audit log の代替にすること。
- Web UI のために core authority を二重化すること。
- hidden server state を LLM context に直接注入すること。
- multi-tenant auth/billing/secret/security を shortcut して実装すること。

## Success criteria / exit conditions

- Workspace / RepositoryId / RepositorySelector / RepositoryPoint / working directory / Runtime / Worker / Control Plane / Ticket / Memory の境界が文書化されている。
- Ticket が team coordination record として、target selector / Artifact / Actor / Permission / Audit と分離された model を持つ。
- `.yoi` local backend は compatibility/local backend として整理され、server-side canonical backend の設計を阻害しない。
- Web UI/API が Ticket / Objective / Runtime / Worker state を中心とした read-only view を提供できる設計または MVP を持つ。Memory は既存 record の表示または将来 placeholder に留め、本格再設計をこの段階の必須条件にしない。
- Control plane から local Runtime に対して、現在のローカル管理画面相当の安全な操作を実行できる design/protocol がある。
- Runtime は single Workspace / Git repository root 専用 process ではなく、sandbox/authority が成立すれば複数 Workspace / Repository の Worker を抱えられる execution substrate として設計されている。
- Git Repository root に依存しない Workspace model があり、Git Repository は Repository provider の一種として扱われている。
- Ticket と Objective は Workspace 配下に平たく存在し、Repository への所属ではなく RepositoryId / RepositorySelector / path scope / intent で対象を表現する。
- Git worktree 相当は working directory materialization strategy として扱われ、Artifact/evidence が concrete RepositoryPoint を記録する。
- Memory は Ticket / Artifact の authority を置き換えない record として platform contract だけを持つ。本格的な意味論・抽出・承認・検索・staleness 処理は、Memory の保存先を Workspace backend / control plane record に移すタイミングで回収する。
- Hosted Runtime / resource allocation / SaaS offering に進むための後続 Ticket が切れる状態になっている。
- 既存 local dogfooding runtime を壊さず、local use と remote-capable architecture が両立している。

## Decision context

- Yoi は hosted Git tool ではなく、team workspace control plane + Runtime execution environment として設計する。
- Team-space の長期 canonical authority は server-side control plane に置く。local `.yoi` は互換/local/offline/export/import surface だが、multi-user SaaS の正本ではない。
- 実行環境と管理システムは弱結合にする。まず管理システムを独立させ、local Runtime を実行環境として接続する。その後に remote/self-hosted/hosted runtime fleet へ進む。
- Runtime は Worker 群を束ねる実行基盤であり、将来的には作業環境の用意、sandbox、mount、checkout/worktree/container filesystem、cache、secret boundary を Worker launch request ごとに準備する。Runtime process は single Workspace / Git repository root 専用に固定しない。RepositorySelector は provider-specific な未解決 locator、RepositoryPoint は解決済み evidence として扱う。
- Web frontend を最初の primary team UI とする。Desktop app は web/control-plane model が安定した後に検討する。
- Git は重要な Repository provider / materialization backend として使うが、Workspace identity と authority を Git Repository root に固定しない。
- Ticket と Objective は Workspace 配下に平たく持つ。対象コードベースや地点指定は RepositoryId / RepositorySelector / path scope / intent として表現し、Worker launch materialization が concrete RepositoryPoint に解決する。
- Backend Repository は cwd inspection ではなく、Workspace config の明示 Repository registry から構築する。`--workspace` は Repository ではなく workspace config root / local descriptor root を指す。
- `./.yoi` は local descriptor / fs-store / compatibility surface であり、将来の Backend canonical store と Workspace registry は `~/.yoi` 側へ寄せる。
- Memory の本格再設計は後回しにする。先に Workspace / Ticket / Repository / Runtime/Worker live view / Control plane の基盤を固め、Memory の保存先を Workspace backend に移すタイミングで、意味論・抽出・承認・検索・staleness 処理をまとめて回収する。
- Worker の一元管理・データ永続化・アーカイブも後続設計に回す。初期 DB では Worker を Pod metadata の代替として永続化せず、live view と Ticket-linked WorkerRef 記録に留める。
