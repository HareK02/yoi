---
title: "Team workspace control plane and runner architecture"
state: "active"
created_at: "2026-06-20T14:26:29Z"
updated_at: "2026-06-21T06:57:06Z"
linked_tickets: ["00001KVMFFYVX"]
---

## Goal

Yoi を、単一のローカル開発ディレクトリで動くエージェント実行ツールから、チームで作業・判断・実行結果を管理できるワークスペース基盤へ発展させる。

この Objective の中心は、Web から扱える管理システムを作り、その管理システムにローカル実行環境・リモート実行環境・将来のクラウド実行環境を接続できるようにすることである。管理システムは Ticket、Objective、Memory、Knowledge、Run、Artifact、Runner の正本を持つ。実行環境はその管理システムから仕事を受け取り、コード取得、作業用ディレクトリ作成、エージェント実行、検証、結果報告を行う。

この Objective は Git ホスティングサービスを作るものではない。Git は重要な Repository provider として扱うが、Yoi の Workspace は Git Repository root と同じものにしない。Yoi が作るべきものは、コード・ドキュメント・データ・成果物などの Repository と実行環境を接続しながら、人間とエージェントの作業、Ticket lifecycle、Memory/Knowledge、検証証跡、実行環境配置を管理するチームワークスペースである。

## Glossary

この Objective では、以下の語をこの意味で使う。

- Workspace: チームまたはプロジェクトの管理単位。Ticket、Objective、Memory、Knowledge、Run、Artifact、Policy、Actor、Repository を持つ。Git Repository root ではない。
- Control plane: Workspace の正本を持ち、Web UI / API / CLI から操作される管理システム。
- Runner: Control plane から仕事を受け取り、実際にエージェントやツールを動かす実行環境。最初はローカルマシン上の runner、後でリモート runner やクラウド runner を追加する。
- Repository: Workspace に接続される source/storage。コード、ドキュメント、local directory、object storage、artifact store、dataset などを含む。Git Repository も Repository の一種であり、基本的には filesystem path ではなく URI / URL で識別する。
- Repository provider: Repository の種類ごとの実装。Git、local filesystem、object store、artifact store、将来の non-Git VCS など。
- RepositoryPoint: Repository 内の特定地点。Git commit/ref/path、object store version/prefix、file snapshot/path など、provider ごとの revision/ref/snapshot/path を表す。
- Execution Workspace: Runner が Run のために作る作業用ディレクトリや container filesystem。1 つ以上の RepositoryPoint から materialize される。Git worktree、clone、sparse checkout などはこれを作る手段である。
- Ticket: チームで扱う作業単位。目的、要件、判断、議論、完了条件、関係、証跡を持つ。
- Objective: 複数の Ticket を束ねる長期目標や設計方針。
- Run: Ticket や Objective に対して行われた具体的な実行試行。どの Runner が、どの Execution Workspace で、何を実行し、どんな結果になったかを持つ。
- Artifact: Run や Ticket に紐づく成果物や証跡。diff、log、validation result、review result、report など。
- Memory: エージェントやユーザーが再利用するための要約された文脈。Ticket や Run の正本ではない。
- Knowledge: 保守された知識や設計判断。Memory より人間が維持する資料に近い。
- Actor: 人間、エージェント、システム、外部サービスなど、Workspace 上で操作や発言を行う主体。

## Motivation / background

現在の Yoi は、ローカルの `.yoi` ディレクトリ、ローカルプロセス、Ticket ファイル、ワークツリー運用によって、自分自身の開発に使えるエージェント実行環境になっている。しかし、チーム利用、Web UI、リモート実行、クラウド実行、最終的な SaaS 提供を考えると、次の前提を変える必要がある。

- Workspace を Git Repository root と同一視しない。
- ローカル filesystem 上の `.yoi` を、長期的なチーム用正本 store にしない。
- Ticket をローカル作業メモではなく、チームの作業調整 record にする。
- Ticket と実行試行を分ける。実行試行は Run として記録する。
- 管理システムと実行環境を分ける。
- まず Web から Ticket、Objective、Memory、Knowledge、Run、Artifact、Runner state を見られるようにする。
- 最初はローカルマシンを Runner として使い、後でリモート Runner、クラウド Runner、runner pool、resource allocation、quota、billing、sandboxing に拡張する。
- Git ホスティング機能を取り込むのではなく、Git Repository / worktree / clone は Repository provider と Execution Workspace materialization の手段として扱う。

OSS として Control plane、Runner、Web frontend、protocol を公開しつつ、managed service では hosted control plane、runner fleet、リソース柔軟性、team auth、backup、audit、availability、multi-tenant operations で価値を出す。

## Strategy / design direction

### 1. Control plane を先に作る

Team Workspace の正本は server-side control plane に置く。`.yoi` は local backend、single-user/self-hosted compatibility、offline/export/import、runner-local projection、migration bridge として残せるが、multi-user SaaS の正本とはみなさない。

Control plane は Ticket、Objective、Memory、Knowledge、Run、Artifact、Actor、Permission、Audit、Runner state を管理する。Web UI、CLI、TUI、将来の desktop client は、この Control plane を操作する client であり、別の正本 store を持たない。

### 2. Workspace と Repository を同一視しない

Workspace はチームまたはプロジェクトの作業管理単位である。Repository は Workspace に接続される source/storage である。Git Repository は Repository の一種にすぎない。

1 つの Workspace は複数の Repository を持てる。Repository は filesystem path ではなく URI / URL で識別する。例として `git+https://...`、`file://...`、`s3://...`、`artifact://...`、将来の VCS provider URI などを扱えるようにする。

Ticket と Objective は Repository 配下に置かず、Workspace 配下に平たく持つ。Ticket は必要に応じて対象 Repository、ref selector、path、必要 capability を持つ。Objective は複数 Ticket にまたがる target default / scope hint を持てるが、Repository の所有物にはしない。

Run は Ticket の target selector を具体的な RepositoryPoint に解決し、その RepositoryPoint から Execution Workspace を materialize する。Git worktree 相当の機能は、この Execution Workspace を作るための実装戦略として扱う。

短期的には Git を主な Repository provider とする。ただし Yoi の authority model を Git object、Git branch、Git Repository root、worktree path に固定しない。Orchestration は Git そのものではなく、`resolve_ref`、`materialize`、`diff`、`patch`、`commit`、`merge` などの Repository capability に依存する。

### 3. Ticket を team coordination record にする

Ticket は実行そのものではない。Ticket は「何を、なぜ、どの条件で完了とみなすか」を持つ。Ticket は Workspace に平たく所属し、Repository には所属しない。コードやドキュメントを対象にする Ticket は、対象 Repository / ref selector / path / intent を target として持つ。

Ticket target は intent/selector であり、実行再現性のための immutable point ではない。Run が target selector を concrete RepositoryPoint に解決し、実際にどの revision/snapshot を materialize したかを記録する。

```text
Ticket
  -> target selectors: Repository + ref selector + path + intent
  -> Run / Attempt
  -> resolved RepositoryPoint
  -> Execution Workspace
  -> Artifact / Evidence
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

Run inputs:
  - repository: main-code
    requested_ref: develop
    resolved_point: git commit abc123
    mount: /workspace/main-code
```

Ticket には次の概念が必要になる。

- Actor identity: human / agent / system / service account.
- Assignment / owner / reviewer / watcher.
- Typed thread events: comment, decision, plan, review, implementation report, state transition.
- Linked Objective / Artifact / Run / Repository / RepositoryPoint / Execution Workspace.
- Permission / visibility.
- Audit trail.
- Notification / mention.
- Board / queue / planning / review / done / archived views.
- Conflict handling and concurrent editing policy.

### 4. Memory / Knowledge の本格再設計は後回しにする

Memory / Knowledge は Ticket / Run / Artifact のコピーではない。再利用可能な文脈、方針、学習された制約、保守された知識として扱う。ただし、Memory の意味論・抽出・承認・検索・staleness 処理を今この Objective で先に作り込まない。

理由は、Memory の正しい設計が Workspace control plane の record model、Actor / visibility / permission、Ticket と Run の分離、Artifact / evidence、RepositoryPoint、Runner に渡す context の監査方法に依存するためである。これらが固まる前に Memory schema だけを作ると、local `.yoi` 前提や現行 agent runtime 前提に引っ張られ、後で再設計が必要になる。

この Objective では、Memory / Knowledge について以下の platform contract だけを維持する。

- Memory / Knowledge は Control plane が扱う record だが、Ticket / Run / Artifact の authority を置き換えない。
- 将来、Memory / Knowledge の canonical storage は Workspace control plane 側に置く。
- local `.yoi` memory は compatibility、offline/export/import、runner-local projection、migration bridge として扱う。
- Personal Memory、Workspace Memory、Run Summary、Maintained Knowledge は分離が必要である。
- Generated Memory には provenance、visibility、approval、audit が必要である。
- Runner / agent に渡した Memory/Knowledge context は、将来 ContextPack などとして Run に記録できる必要がある。

本格的な Memory 再設計は、Memory の保存先を Workspace backend / control plane record に移すタイミングで回収する。それまでは低リスクな観察、問題例の収集、既存 local memory の互換維持に留める。

### 5. 管理システムと実行環境を弱結合にする

Control plane は正本と調整を持つ。Runner は実行を担当する。

初期形:

```text
Web UI / Control Plane
  -> Runner connection
  -> Local machine runner
  -> Existing Yoi runtime, tools, working copy, build/test commands
```

この段階では、現在ローカル管理画面が行っている Ticket 選択、エージェント起動、レビュー起動、作業用 checkout 作成、検証実行、結果表示を、Web/control plane から local runner に対して実行できるようにする。

その後で、remote runner、self-hosted runner、hosted cloud runner、runner pool、resource allocation、quota、billing、sandbox、network policy、secret distribution を追加する。

```text
Phase 1: Web control plane + local runner
Phase 2: Remote/self-hosted runner
Phase 3: Hosted cloud runner fleet
Phase 4: Resource allocation / scheduling / quotas / billing / isolation
```

### 6. Web frontend を先に作る

Desktop app は対応コストが高いので、まず Web frontend を primary UI とする。

- Web: チームで使う主要 UI。
- CLI: automation、scripting、local operations。
- TUI/local panel: local runner cockpit、fallback、dogfooding surface。
- Future desktop: Web/control-plane model が安定した後に検討する optional client。

Web UI は Ticket、Objective、Memory、Knowledge、Run、Runner、Artifact を扱う。UI の都合で正本を二重化しない。

### 7. 多重起動コストと runtime placement を見直す

Cloud/remote execution を成立させるには、多数のエージェント実行を安く管理できる必要がある。logical agent session と runtime process/resource placement を分ける。

検討対象:

- Agent identity と process/runtime placement の分離。
- Provider client、tool registry、resource cache の共有可能性。
- Prompt/resource/profile resolution cache。
- Model call multiplexing and scheduling。
- Tool execution sandbox reuse。
- Plugin instance / Service runtime との統合。
- Session/event stream と runtime lifecycle の分離。
- Runner-local cache、checkout reuse、build cache、dependency cache。

## Initial phases / candidate tickets

1. **Vocabulary / architecture record**
   - Workspace / Repository / RepositoryPoint / Execution Workspace / Runner / Control Plane / Run / Ticket / Memory / Knowledge の用語と境界を固める。
2. **Team-space canonical data model**
   - Ticket / Objective / Target / Run / Artifact / Actor / Permission / Audit / Memory / Knowledge の entity/event model を設計する。
3. **Ticket and Run separation**
   - Ticket lifecycle と execution attempt / orchestration run / validation run を分離し、Ticket thread と Run evidence の責務を明確化する。
4. **Memory storage migration boundary**
   - Memory / Knowledge の本格再設計は後回しにし、まずは Workspace backend に移す時の platform contract、compatibility/cache/export 方針、将来の provenance / visibility / approval 要件だけを固定する。
5. **Control plane backend architecture**
   - local `.yoi` backend と server-side canonical backend の境界、migration/export/import、compatibility mode を設計する。
6. **Web control plane MVP design**
   - read-only Ticket / Objective / Memory / Knowledge / Runner state UI/API の範囲を決める。
7. **Local runner protocol design**
   - Web/control plane から local runner に安全な操作を送る protocol と authority boundary を設計する。
8. **Repository and Execution Workspace materialization model**
   - Repository URI、Repository provider capability、RepositoryPoint resolution、Git worktree / clone / sparse checkout / future source backend を runner-side strategy として抽象化する。
9. **Remote/hosted runner foundation**
   - runner registration, heartbeat, capability advertisement, job assignment, logs/events, secrets, sandbox/resource policy を設計する。

## Non-goals

- Git hosting service を作ること。
- `.yoi` filesystem をそのまま SaaS canonical store にすること。
- 最初から full hosted cloud execution を作ること。
- local execution / CLI / TUI / local panel を捨てること。
- Ticket を単なる issue tracker clone にすること。
- Memory を Ticket/Run audit log の代替にすること。
- Web UI のために core authority を二重化すること。
- hidden server state を LLM context に直接注入すること。
- multi-tenant auth/billing/secret/security を shortcut して実装すること。

## Success criteria / exit conditions

- Workspace / Repository / RepositoryPoint / Execution Workspace / Runner / Control Plane / Run / Ticket / Memory / Knowledge の境界が文書化されている。
- Ticket が team coordination record として、target selector / Run / Artifact / Actor / Permission / Audit と分離された model を持つ。
- `.yoi` local backend は compatibility/local backend として整理され、server-side canonical backend の設計を阻害しない。
- Web UI/API が Ticket / Objective / Runner state を中心とした read-only view を提供できる設計または MVP を持つ。Memory / Knowledge は既存 record の表示または将来 placeholder に留め、本格再設計をこの段階の必須条件にしない。
- Control plane から local runner に対して、現在のローカル管理画面相当の安全な操作を実行できる design/protocol がある。
- Git Repository root に依存しない Workspace model があり、Git Repository は Repository provider の一種として扱われている。
- Ticket と Objective は Workspace 配下に平たく存在し、Repository への所属ではなく target selector / scope hint で対象を表現する。
- Git worktree 相当は Execution Workspace materialization strategy として扱われ、Run が immutable な RepositoryPoint を記録する。
- Memory / Knowledge は Ticket / Run / Artifact の authority を置き換えない record として platform contract だけを持つ。本格的な意味論・抽出・承認・検索・staleness 処理は、Memory の保存先を Workspace backend / control plane record に移すタイミングで回収する。
- Hosted runner / resource allocation / SaaS offering に進むための後続 Ticket が切れる状態になっている。
- 既存 local dogfooding runtime を壊さず、local use と remote-capable architecture が両立している。

## Decision context

- Yoi は hosted Git tool ではなく、team workspace control plane + execution environment として設計する。
- Team-space の長期 canonical authority は server-side control plane に置く。local `.yoi` は互換/local/offline/export/import surface だが、multi-user SaaS の正本ではない。
- 実行環境と管理システムは弱結合にする。まず管理システムを独立させ、local runner を実行環境として接続する。その後に remote/self-hosted/hosted runner fleet へ進む。
- Web frontend を最初の primary team UI とする。Desktop app は web/control-plane model が安定した後に検討する。
- Git は重要な Repository provider / materialization backend として使うが、Workspace identity と authority を Git Repository root に固定しない。
- Ticket と Objective は Workspace 配下に平たく持つ。対象コードベースや ref は Repository target selector として表現し、Run が concrete RepositoryPoint に解決する。
- Memory の本格再設計は後回しにする。先に Workspace / Ticket / Run / Repository / Runner / Control plane の基盤を固め、Memory の保存先を Workspace backend に移すタイミングで、意味論・抽出・承認・検索・staleness 処理をまとめて回収する。
