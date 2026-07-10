---
title: "Runtime working directory materialization and sandboxed agent environments"
state: "active"
created_at: "2026-07-06T16:28:12Z"
updated_at: "2026-07-10T16:50:00Z"
linked_tickets: ["00001KWPC13WQ", "00001KWMBAA6V", "00001KX6BPY7M", "00001KX6CRVBE"]
---

## Goal

Runtime が Worker ごとに安全で安価な作業環境を用意できるようにする。Yoi の Runtime は、単に既存ディレクトリで Worker process を起動する launcher ではなく、RepositoryPoint から working directory を materialize し、sandbox / mount / cache / cleanup / evidence を管理する実行基盤になる。

この Objective の中心は、Worker 用 working directory の materialization、Repository cache、working directory allocation、sandboxed agent environment の境界を設計し、将来的に 1 つの Runtime が複数 Workspace / Repository の Worker を抱えられるようにすることである。

初期実装では Git/local repository を主対象にしてよい。ただし設計は Git worktree 固定にしない。Git worktree、bare object cache、sparse checkout、copy-on-write snapshot、reflink copy、APFS clonefile、btrfs snapshot、overlay filesystem、container filesystem、remote object snapshot は、すべて working directory materialization strategy の候補として扱う。

## Motivation / background

現在の Runtime は `--workspace` で与えられた単一 root を Worker の workspace scope として使う。この形では次の問題がある。

- 複数 Worker が同じ repository root を scope として要求すると allocation conflict が起きる。
- Runtime が single Workspace / Git repository root 専用 process になってしまう。
- Worker が source repository root を直接触るため、sandbox / cleanup / quota / evidence の境界が曖昧になる。
- Worker ごとに full clone すると容量と時間のコストが大きすぎる。
- `.yoi` / Backend fs-store / Workspace descriptor と、Worker 実行用 checkout / scratch / build output の lifecycle が混ざりやすい。

Yoi は Workspace / Repository / Runtime を分ける方針になっている。Backend は Repository registry、RepositorySelector、RepositoryPoint、Ticket/Artifact evidence の authority を持つ。一方 Runtime は RepositoryPoint を受け取り、Worker が使う working directory を用意する責務を持つべきである。

したがって、Repository を持ってくる実体ディレクトリは Backend / Workspace store ではなく Runtime 管理領域に置く。`./.yoi` は local descriptor / compatibility / project record surface であり、Worker ごとの checkout、worktree、snapshot、sandbox root、build cache、dependency cache を置く場所ではない。

参考になる方向性として、Rift のような copy-on-write workspace snapshot / reflink / btrfs snapshot / APFS clonefile による安価な workspace creation がある。ただし Yoi は Rift を Git worktree 代替 CLI として直接前提にするのではなく、Runtime materializer backend の一候補として扱う。

## Glossary

- Runtime root: Runtime が自身の store、cache、Worker metadata、working directory allocation を管理する root。長期的には `~/.yoi/runtimes/<runtime-id>/` 配下など、Workspace backend store とは別に置く。
- Repository cache: Runtime-local の共有 source/cache。Git なら bare mirror / object cache / packfile cache など。重く、長寿命で、複数 Worker allocation から共有される。
- working directory: Worker ごとの作業環境。短寿命で、Worker が読み書きする root / mounts / scratch / overlay を含む。Browser-facing UI では `workspace` と混同しないよう `workdir` と表示してよい。`Volume` は storage backing の候補名であり、この作業領域そのものの呼称にはしない。
- Worker record: 作業単位として保存する record。profile、Runtime、RepositoryPoint / workdir evidence、session/transcript refs、status、diagnostics、summary、pinned flag を束ねる。Session 単体ではなく Worker を保存・削除の単位にする。
- Materialization strategy: RepositoryPoint から working directory を作る実装戦略。Git detached worktree、sparse checkout、CoW snapshot、reflink copy、overlay、container filesystem など。
- WorkingDirectoryAllocation: Runtime が払い出した作業環境の record。allocation id、worker id、workspace id、repository point、materializer kind、root、mounts、cleanup policy、status を持つ。
- Sandbox policy: Worker が見られる filesystem、network、process、secret、tool authority の境界を表す policy。
- Dirty state policy: local uncommitted changes を Worker environment に含めるかどうかの方針。clean point only、patch artifact apply、snapshot current worktree など。

## Strategy / design direction

### 1. Backend store と Runtime execution storage を分ける

Workspace/backend store は canonical or local descriptor records を扱う。

```text
Backend / Control plane store
  - Workspace registry
  - Repository registry
  - Ticket / Objective
  - Artifact metadata / evidence
  - Actor / Permission / Audit
  - Runtime registry / observed state
```

Runtime execution storage は Worker 実行のための materialized filesystem と cache を扱う。

```text
Runtime root
  - runtime catalog / runtime-local DB
  - config bundles
  - worker metadata / transcript references
  - repository-cache
  - working-directories
  - sandbox state
  - build/dependency cache
  - cleanup ledger
```

この 2 つを混ぜない。特に `.yoi` や workspace config root に Worker ごとの checkout/worktree/sandbox root を置かない。

推奨配置の方向:

```text
~/.yoi/
  workspace-server/
    backend.db
    workspaces/
      <workspace-id>/
        workspace.db
        artifacts/

  runtimes/
    <runtime-id>/
      runtime.db
      config-bundles/
      workers/
        <worker-id>/
          metadata/
      repository-cache/
        git/
          <repository-cache-key>/
            bare.git
      working-directories/
        <allocation-id>/
          root/
          mounts/
          scratch/
          materialization.json
      build-cache/
      tmp/
```

### 2. clone ではなく materialize と呼ぶ

Runtime は Repository を毎回 clone するのではない。Runtime は RepositoryPoint を Worker 用の working directory として materialize する。

```text
RepositoryId + RepositorySelector
  -> resolved RepositoryPoint
  -> Runtime repository-cache
  -> WorkingDirectoryMaterializer
  -> WorkingDirectoryAllocation
  -> Worker process
```

Git の場合でも materialization strategy は複数あり得る。

- shared bare cache + detached Git worktree
- sparse checkout
- partial clone / blob filter
- reflink copy
- btrfs writable snapshot
- APFS clonefile
- overlay filesystem
- external tool backed snapshot, e.g. Rift-like CoW workspace creation

### 3. Repository cache と Worker working directory を分離する

full clone を Worker ごとに作らない。重い source/object data は Runtime-local repository cache に集約し、Worker ごとの working directory は cheap allocation にする。

Git v0 の方向:

```text
repository-cache/git/<repo-key>/bare.git
working-directories/<allocation-id>/root/<repository-id>/
```

初回:

```text
git clone --mirror <uri> repository-cache/git/<repo-key>/bare.git
```

次回以降:

```text
git -C repository-cache/git/<repo-key>/bare.git fetch --prune
```

Worker allocation:

```text
git --git-dir=<bare.git> worktree add --detach <execution-root>/<repository-id> <commit>
```

Git branch 名を直接 checkout しない。Git worktree は同一 branch を複数 worktree に checkout しづらいため、RepositorySelector を RepositoryPoint に解決し、resolved commit を detached worktree として materialize する。Worker が branch を必要とする場合は Worker/Task ごとの synthetic branch を別途作る。

### 4. Dirty state を明示 policy にする

local dirty changes を暗黙に Worker に見せない。

可能な policy:

- `clean_point_only`: dirty workspace は materialization 拒否。再現性が高く v0 の default 候補。
- `patch_artifact`: clean RepositoryPoint を materialize し、Backend が保存した dirty diff artifact を apply する。
- `current_worktree_snapshot`: CoW/reflink/Rift-like snapshot で現在の working tree を snapshot として materialize する。便利だが evidence と cleanup の扱いを明示する必要がある。
- `direct_legacy_mount`: 既存 root をそのまま渡す。debug/legacy only。通常 Worker creation の default にしない。

Dirty state を含める場合、Artifact/evidence には source RepositoryPoint、patch/snapshot digest、created_at、materializer kind を残す。

### 5. Sandbox を materialization と同じ境界で扱う

working directory は単なる directory path ではなく、Worker が見てよい filesystem view である。Runtime は working directory allocation と同時に sandbox/mount/authority を構築する。

Worker に渡すもの:

- workspace root
- repository mounts
- scratch/cache dirs
- tool authority
- env vars
- config bundle
- secret handles, not raw secrets

Worker が自分で発見してはいけないもの:

- host repository root
- Backend store path
- `.yoi` authority-bearing internals
- raw credentials
- Runtime socket/store/cache internals
- sibling Worker working directories

Sandbox v0 は strong isolation でなくてもよい。ただし型と lifecycle は、後で container sandbox、namespace, mount filtering, network policy, secret boundary に拡張できる形にする。

### 6. working directory registry を持つ

Runtime は materialized workspace を filesystem だけでなく registry でも管理する。

必要な record:

```text
WorkingDirectoryAllocation
  id
  runtime_id
  worker_id
  workspace_id
  repository_points[]
  materializer_kind
  root
  mounts[]
  scratch
  source_cache_refs[]
  sandbox_policy
  dirty_state_policy
  cleanup_policy
  status: active | stopped | cleanup_pending | removed | failed
  created_at
  stopped_at
  diagnostics[]
```

この registry は Runtime root 側に置く。Backend は必要な evidence と summary だけを受け取る。Browser-facing API は raw host paths を原則漏らさない。

### 7. Heavy regenerable artifacts は policy で除外または cache 化する

Worker working directory creation では、`node_modules`, `target`, `.venv`, framework cache, dist, build, coverage などを無条件に full copy しない。

Materialization policy は以下を持てるようにする。

- exclude regenerable artifacts
- include manifests / lockfiles
- share dependency cache
- use build cache
- path scope / sparse checkout
- per-repository materialization options

Rift の filtered CoW creation のように、重い artifacts を除外しながら source tree を高速に用意できる strategy を将来取り込めるようにする。

## Initial implementation phases

### Phase 1: Materialization boundary and legacy allocation record

- `CreateWorkerRequest` に working directory request / target placeholder を追加する。
- `WorkingDirectoryMaterializer` trait を `worker-runtime` に追加する。
- `WorkerRuntimeExecutionBackend` が Worker spawn 前に materializer を呼ぶ順序にする。
- v0 materializer は existing local root を explicit allocation として返してよいが、`direct_legacy_mount` として明示し、通常設計の final form と混同しない。
- Allocation record / cleanup policy / diagnostics の型を先に作る。
- Worker が source repository root を直接 scope として要求する経路を deprecated/legacy に閉じ込める。

### Phase 2: Runtime root and working directory storage

- `--runtime-root` を導入し、Runtime state / repository-cache / working-directories / worker metadata / worker runtime dirs を Runtime root 配下へ寄せる。
- `--workspace` は legacy bootstrap input としてだけ扱い、Runtime identity / long-term workspace binding から外す。
- Runtime root default は user data 配下にする。
- working directory allocation registry を Runtime root に保存する。

### Phase 3: Git cached detached worktree materializer

- Git repository cache を Runtime root に作る。
- RepositorySelector を RepositoryPoint に解決する呼び出し境界を作る。
- resolved commit/tree を evidence として残す。
- Worker ごとに detached worktree を `working-directories/<allocation-id>/root/<repository-id>` に作る。
- Worker stop / cleanup 時に `git worktree remove` と registry cleanup を行う。
- dirty state は v0 では `clean_point_only` を default にし、dirty local workspace は明示 diagnostic で拒否する。

### Phase 4: Path scope / sparse checkout / cache policies

- Ticket target / Worker launch request の path scope を materialization に渡す。
- Git sparse checkout を materializer strategy として追加する。
- heavy artifact exclude / dependency cache / build cache policy を導入する。

### Phase 5: CoW / snapshot materializer

- btrfs snapshot、Linux reflink、macOS APFS clonefile、Rift-like snapshot backend を materializer strategy として検討・実装する。
- `current_worktree_snapshot` policy を evidence と cleanup 付きで扱う。
- source root と generated workspace の registry / ancestor / cleanup model を設計する。

### Phase 6: Strong sandbox and remote Runtime support

- container filesystem / mount namespace / network policy / secret handle / tool authority を Runtime allocation と統合する。
- Runtime が複数 Workspace / Repository の Worker を同時に抱える場合の namespace、quota、cleanup、audit boundary を固める。
- remote/self-hosted/hosted Runtime fleet で repository cache と working directory storage をどう扱うかを設計する。

### 7. Worker / Session / workdir retention and cleanup policy

- Worker を保存・削除単位にする。Session / transcript は Worker に内包または参照される履歴として扱い、Session 単体を長期保存 authority にしない。
- Worker lifecycle は `running -> stopped -> delete` を基本にする。`archived` を lifecycle state として導入しない。
- Worker には `pinned` flag を持たせる。Pinned Worker は manual delete / cleanup / future automatic prune から守られる。
- Worker delete は Worker record と内包する Session / transcript history の削除を意味する。Workdir files は自動削除しない。
- Session / transcript は削除可能にする。圧縮 archive storage や高度な summarized retention は必要になった時点で別の storage policy として設計する。
- Workdir 実ファイルは durable history ではなく再現可能 cache として扱う。RepositoryPoint / resolved commit から再現でき、dirty/uncommitted state が無いなら、running Worker に紐づかない workdir files は manual cleanup eligible とする。
- Dirty Workdir は活動中または要判断状態として扱う。削除は可能だが、changes ごと消す explicit confirmation を要求する。Dirty orphan は recovery Worker 起動または explicit discard の判断対象であり、通常の clean cleanup と区別する。
- Workdir record は materialization evidence として扱う。実ファイルが削除済みなら `removed`、外部要因で欠落しているなら stale/missing materialization として診断可能にする。
- Worker と Workdir の関係は Backend registry の link table を authority にする。Worker record は最後に活動した RepositoryPoint / resolved commit / workdir binding summary を保持し、Stopped Worker + Removed Workdir でも必要に応じて再 materialize できるようにする。
- Cleanup/delete は当面 manual-first にする。削除前に対象、理由、削除される bytes、blocking conditions（running Worker、pinned Worker、dirty state、unreachable commit など）を plan として提示する。
- 将来的には容量/期限ベースの automatic prune を設定可能にする予定。ただし automatic prune は user-configured policy と pinned protection を前提にし、初期の manual cleanup/delete とは別段階で扱う。
- UI 表示は `workdir` に寄せる。内部型/API の互換名 `working_directory` は移行中に残ってよいが、Browser-facing navigation では Runtime 管理配下の `Workdirs` として扱う。

## Non-goals

- v0 で完全な container sandbox を実装すること。
- Worker ごとに full clone すること。
- `.yoi` や Backend workspace store に Worker working directory を置くこと。
- Git worktree を唯一の materialization strategy として固定すること。
- Dirty local changes を暗黙に Worker に渡すこと。
- Browser-facing API に raw host path、secret、Runtime internal store path を公開すること。
- Repository credential / secret distribution の本格設計をこの Objective だけで完了させること。

## Success criteria / exit conditions

- Runtime root、Repository cache、working directory、Backend/Workspace store の境界が文書化されている。
- Runtime が Worker spawn 前に working directory materializer を呼ぶ型と順序を持つ。
- Worker は source repository root ではなく materialized working directory を scope として起動する。
- working directory allocation が Runtime registry に記録され、cleanup policy を持つ。
- Git/local repository の v0 materializer が full clone 連発ではなく shared cache / detached worktree / cheap allocation の方向に進んでいる。
- dirty state policy が明示され、clean point、patch artifact、snapshot のどれを使ったか evidence に残せる。
- Runtime process 起動時の `--workspace` は legacy bootstrap input として隔離され、Runtime identity や single workspace binding とみなされない。
- Worker ごとの scope allocation conflict が、同一 source root を直接渡す設計ではなく materialized workspace allocation によって解消される。
- Sandbox / mount / cache / secret boundary を後続実装で強化できる model になっている。
- Worker record が Session / transcript refs と pinned flag を束ね、`pinned` Worker を cleanup/delete/future automatic prune から守れる。
- Workdir 実ファイルは再現可能 cache として manual cleanup/delete でき、削除前に plan-first で linked Worker、session retention、commit reachability、dirty/missing 状態を確認できる。
- 将来的に容量/期限ベースの automatic prune を user-configured policy として追加できる設計余地がある。

## References

- [Rift: Worktree alternative for fast copy-on-write workspaces](https://github.com/anomalyco/rift) — CoW snapshot / reflink / btrfs snapshot / APFS clonefile による高速 workspace creation、heavy regenerable artifacts の除外、workspace registry などを Runtime materializer backend 設計の参考にする。

## Decision context

- Runtime は Worker 群を束ねる実行基盤であり、Worker 用の作業環境を用意する責務を持つ。
- Repository は clone されるものではなく、RepositoryPoint から Worker 用 working directory として materialize される。
- Runtime execution storage は Backend/Workspace store と分離する。`.yoi` は Worker working directory 置き場ではない。
- full clone を Worker ごとに作らない。Runtime-local repository cache と Worker-local cheap workspace allocation を分ける。
- Git detached worktree は v0 materialization strategy として有力だが、Git worktree 固定の設計にはしない。
- CoW snapshot / reflink / btrfs snapshot / APFS clonefile / Rift-like workspace creation は、将来の materializer backend として検討する。
- Dirty local state は暗黙に渡さず、policy と evidence を持って扱う。
- Sandbox は後付けの別機能ではなく、working directory allocation と同じ境界で扱う。
