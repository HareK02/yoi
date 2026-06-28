---
title: 'Runtime Worker起動経路を正規のExecution/ConfigBundle経路に一本化する'
state: 'ready'
created_at: '2026-06-28T13:35:21Z'
updated_at: '2026-06-28T16:41:17Z'
assignee: null
---

## 背景

Runtime 上の Worker 起動経路が増え、一覧には出るが入力できない Worker、execution backend につながっていない Worker、再起動で消える Worker が作れてしまう状態になっている。

Worker を作る手順は、embedded、remote、Workspace Companion、Web Console のどれから呼ばれても同じであるべき。違いは配置や通信方式に閉じ込め、Worker 作成そのものは正規の API を通す。

この Ticket は、最終 API の field を疑似定義するものではない。`CreateWorkerRequest` などの具体 field は、既存型・永続化・retry・transport・scope 境界を確認したうえで実装時に確定する。ここでは API 境界、責務、検証すべき性質を定義する。

## 目的

- Runtime 上の Worker 作成手順を 1 本にする。
- 作られた Worker は、入力・観測・実行・履歴保存がそろった状態にする。
- Workspace Backend / Companion / Web Console が独自に Worker 風のものを作らないようにする。
- Runtime に workspace filesystem access / tool execution scope を持たせない。
- working directory や Ticket / Objective の意味は Backend 側で扱い、Runtime の Worker identity に混ぜない。
- Worker creation を中間状態が残りにくい transactional API にする。
- 未検証の request field を API 案として固定しない。

## やり取りの全体像

外部から見た Worker 起動は、Workspace Backend への 1 回の launch request にする。Browser / Web Console は Runtime の内部 config store、execution binding、cwd、tool scope、secret、Runtime endpoint を知らない。

```text
Browser / Web Console
  -> Workspace Backend: launch worker

Workspace Backend
  -> Runtime: ensure/sync ConfigBundle if needed
  -> execution host / tool host: create execution binding if needed
  -> Runtime: create worker
```

Runtime の Worker creation は 1 request / 1 transaction として扱う。途中で必要な前提が欠けていた場合は typed error を返し、入力不能な Worker や fake response は作らない。

## 検証済み field audit

現在の実装に存在する Worker creation / launch 関連 field は、`artifacts/api-field-audit.md` で調査済み。以後この Ticket で API field を追加・削除・改名する場合は、その artifact と同じ観点で検証してから行う。

主な結論:

- 現行 Runtime `CreateWorkerRequest` の `intent` / `requested_capabilities` / `workspace_refs` / `mount_refs` は canonical Runtime create request には残さない。
- 現行 Browser-facing `WorkerSpawnRequest.config_bundle` / `requested_capabilities` は Runtime create にそのまま流さない。
- ConfigBundle は Browser-facing launch から選ばせず、Backend が決定・sync/check した stable identity だけを Runtime create 境界で扱う。
- `ExecutionBindingRef` は現行実装に存在しないため、最終 field として固定しない。現時点で確定するのは、Runtime create payload に raw cwd / workspace / tool scope を入れない境界だけ。
- initial input を `Vec<Segment>` に寄せる方針は妥当だが、現行 Runtime input / transcript は String なので、field だけ先に固定しない。

## API 境界

### Browser-facing launch

Browser-facing API は product semantics を受け取る。これは Runtime の Worker creation API ではない。

Browser-facing launch で扱ってよい情報:

- Profile selector または role selector。
- role / display name などの表示・選択情報。
- 対象 Ticket / Objective など Backend launch context。
- acceptance policy など、Backend が解釈して Runtime create に落とし込む方針。

Browser-facing launch に出してはいけない情報:

- Runtime endpoint / credential / socket path / session path。
- raw workspace path / cwd。
- tool scope / tool access の具体設定。
- Runtime 内部の config store 状態。
- execution binding の中身。

### ConfigBundle sync

`ConfigBundle` 本体は Worker creation payload に混ぜない。Backend が Profile などから ConfigBundle を決定し、必要なら Runtime に同期する。

ただし、Runtime create request が何を持つかは実装時に検証して決める。特に `ConfigBundleRef` のような参照を渡す案は、request を出す側が Runtime 内部の config store を把握する必要を生む可能性があるため、無条件に採用しない。

API 確定時に満たすべき条件:

- create caller が Runtime 内部 store を探索・選択しなくてよい。
- retry / duplicate launch で同じ ConfigBundle を指せる。
- Runtime restart 後に、どの ConfigBundle で Worker を作ったか診断できる。
- Runtime は create 時に ConfigBundle の存在、内容同一性、Profile compatibility、provider / secret / declaration の整合を検証できる。
- `LatestForProfile` のような時点依存の選択を Worker creation の再現性に混ぜない。

候補としては、Backend が同期した bundle について Runtime から返る stable identity を Runtime create に渡す方式がある。ただしこれは Browser-facing API に出さず、既存の ConfigBundle store / digest 型 / sync API に従って実装時に確定する。

### Execution binding

working directory、cwd、tool scope、Ticket backend access は Runtime に渡さない。embedded Runtime も例外ではなく、Runtime 自体は workspace filesystem access を持たない。

Workspace Backend は launch context をもとに execution host / tool host 側へ binding を作る。binding は execution backend が Worker を起動するときに使う opaque handle であり、Runtime が中身を解釈するものではない。

API 確定時に満たすべき条件:

- binding 作成は Runtime API ではなく、Backend / execution host / tool host 側の内部 API として扱う。
- Runtime は binding の cwd / raw workspace path / tool scope / Ticket backend path を metadata として保持しない。
- Runtime は binding の中身を解釈して file operation を行わない。
- file/tool access の制限は execution backend / tool host 側で行う。
- remote Runtime が local workspace の binding を使えない場合は、binding の中身を Runtime create request に展開せず typed error にする。
- 未使用 binding が残る場合に備え、execution host 側で lease / TTL / explicit cleanup のいずれかを持つ。

`ExecutionBindingRef` という名前は候補であり、最終型名ではない。現時点で専用システムが存在しない概念を API field として新設しない。既存の execution backend 接続型や tool host の scope 設定を確認してから決める。

### Initial input

initial input は Worker creation と同じ transaction で transcript に入る。別 store に先置きする `InitialInputRef` / `PrepareInitialInput` は作らない方針とする。

initial input は独自 message 型ではなく、既存 protocol の `Segment` / user submission 表現に寄せる。`System` message は作らない。role prompt / system-level instruction は Profile / ConfigBundle 側で解決する。

API 確定時に満たすべき条件:

- Ticket / Objective の要約文は通常の text segment として扱える。
- Ticket / Objective など読み直し可能な record は、別 field ではなく Segment の既存または追加 variant で表せる。
- Worker 側では通常の user submission と同じ解決経路で context 化される。
- Backend claim、RuntimeRegistry の振り分け、raw workspace path、working directory の作成方針、execution binding は conversation context に入らない。

既存 `Segment` の variant と変換経路を確認し、不足があればこの Ticket の実装内で Segment 側を拡張するか、別 Ticket に分ける。

### Runtime worker creation

Runtime worker creation は、同期済み ConfigBundle、execution binding、initial input を結びつけ、execution backend に接続した Worker を登録する唯一の経路にする。

ただし、最終的な `CreateWorkerRequest` の field はこの Ticket 本文では固定しない。実装前に各 field を次の観点で検証し、Ticket thread または実装 report に残す。Worker creation の永続化は必須であり、永続化しない worker creation path は作らない。

各 field の検証観点:

- source: 誰がその値を作るか。
- scope/access: その値は既存の access/scope 制限に関わるか、単なる識別子・表示情報か。
- visibility: Browser-facing API / Runtime metadata / Worker conversation / execution backend のどこに見えてよいか。
- persistence/projection: Runtime restart 後に、既存の正規 store のどの record / projection に何を保存し、どの情報を復元・診断に使うか。store を選ぶ項目ではない。`no` は選択肢にしない。raw handle などを保存できない場合でも、対応する durable identity / projection / diagnostic record を持つ。
- retry: retry / duplicate launch で同じ意味になるか。
- validation: Runtime create 時に何を検証できるか。
- existing type: 既存型・既存 protocol に対応物があるか。
- failure: 不足・不整合時にどの typed error になるか。

現時点で確定している制約:

- WorkerId を発行するのは Runtime worker creation だけ。
- Worker creation は 1 request / 1 transaction として扱う。
- Worker creation の結果は worker-runtime fs-store / transcript store / catalog の既存正規 store に永続化される。永続化しない Worker は通常 Worker として作らない。
- Runtime は workspace filesystem access / tool execution scope を持たない。
- Runtime は fake / providerless assistant response を生成しない。
- Runtime は execution backend 未接続の Worker を通常 Worker として公開しない。

## transaction と失敗処理

Runtime worker creation は概ね次を 1 transaction として行う。ただし具体的な保存順序は実装時に worker-runtime fs-store / execution backend / transcript store の既存 contract を確認して決める。永続化しない起動はこの Ticket の対象にしない。

```text
create worker:
  1. retry / duplicate launch を扱う。
  2. ConfigBundle を検証する。
  3. execution binding を execution backend に渡して接続可能か確認する。
  4. WorkerId / TranscriptId を割り当てる。
  5. initial input を user submission として transcript に保存する。
  6. Runtime catalog に Worker を登録する。
  7. execution backend に接続する。
  8. response を返す。
```

途中失敗した場合は、WorkerId を公開しないか、公開済みなら failed creation として明示的に診断できる状態にする。入力不能な Worker や fake response は作らない。

想定される typed error は実装時に既存 error 型と合わせて確定する。少なくとも次の分類は必要になる。

- duplicate / retry conflict。
- ConfigBundle missing / mismatch / incompatible。
- provider / secret / declaration 不足。
- execution binding missing / unsupported。
- execution backend unavailable。
- persistence failure。

## 境界

- Runtime は workspace filesystem access / tool execution scope を持たない。
- embedded Runtime でも Runtime 自体に workspace filesystem access を許可しない。
- raw workspace path / cwd / socket path / credential / session path は Browser-facing API に出さない。
- Ticket id / Objective id は Runtime の Worker identity にしない。
- `ConfigBundle` 本体は Worker creation payload にしない。
- `InitialInputRef` は作らず、initial input は Worker creation request に inline する。
- `ExecutionEnvironmentRef` のように Runtime が working directory materialization を持つように見える型は避ける。
- execution backend に接続できないものを通常 Worker として一覧に出さない。

## 既存経路の整理

- embedded Runtime は上記の作成手順を通る。ただし embedded Runtime に workspace filesystem access を付与しない。
- remote Runtime も同じ worker creation contract を使う。execution binding を使えない段階では typed error で拒否する。
- Companion 専用の作成・送信経路が残る場合も、内部ではこの worker creation API の薄い呼び出しにする。
- Web Console は `runtime_id + worker_id` を対象に入力・観測するだけにし、Worker 作成の特別扱いを持たない。

## 保存と再起動

- Worker 作成時に Worker id、参照した ConfigBundle identity、execution binding identity、transcript id、initial input を保存する。保存しない選択肢は取らない。
- 保存する identity が access/scope を与える値にならないようにする。たとえば execution binding identity は Runtime の filesystem access ではなく、stale execution mapping の診断と execution backend reconnect のための参照として扱う。
- 再起動後に live execution handle を復元できない場合は、stale execution mapping として診断する。
- ConfigBundle、execution binding、履歴のどれかが見つからない場合は typed diagnostic にし、黙って別 Worker を作らない。

## 対象外

- Web Console の見た目変更。
- 複数利用者の auth / permission / redaction policy の完成。
- provider / secret store UI の新設。
- remote Runtime process supervisor の完成。
- remote Runtime の execution provisioning の完成。
- `pod_store` など旧内部 store 名の全面 rename。

## 実装前に必ず確認するもの

初回調査結果は `artifacts/api-field-audit.md` に記録済み。実装時はその調査結果を更新し、コード変更に合わせて以下を再確認する。

- 現在の `worker-runtime` の create / catalog / fs-store / execution backend mapping 型。
- 現在の Workspace Backend launch API と embedded Runtime 呼び出し箇所。
- ConfigBundle store / digest / sync の既存型と責務。
- existing `Segment` / user submission / transcript append の型と変換経路。
- execution host / tool host / scope 設定の既存境界。
- retry / duplicate launch に使える既存 idempotency key または request id。

## 受け入れ条件

- Browser-facing launch request と Runtime worker creation request の責務差が code/docs/tests 上で明確になっている。
- Runtime が workspace filesystem access / tool execution scope を持たないことが code/docs/tests 上で明確になっている。
- ConfigBundle sync と worker creation payload が分離されている。
- worker creation caller が Runtime 内部 config store を探索・選択する必要がない。
- ConfigBundle の識別方法が content identity / digest / 既存 store contract の観点で検証され、thread または実装 report に記録されている。
- execution binding 作成経路が Backend / execution host 側に定義され、Runtime API として扱われていない。
- initial input は Worker creation request に inline され、`InitialInputRef` / `PrepareInitialInput` のような中間 API を作らない。
- initial input は既存 `Segment` / user submission 表現に寄せられ、Ticket / Objective record 参照も Segment 経路で扱われる。
- Runtime worker creation request の各 field について、source / scope-access / visibility / persistence-projection / retry / validation / existing type / failure が検証されている。persistence-projection は保存有無や store 選択ではなく、既存の正規 store のどの record / projection に何を保存・復元・診断するかを確認する項目である。
- Runtime worker creation が 1 request / 1 transaction として扱われ、WorkerId を発行する唯一の API になっている。
- embedded Worker / Workspace Companion / remote-facing Worker creation が同じ作成手順を使う。
- input-capable Worker が execution backend 未接続になる経路がない。
- fake / providerless assistant response を生成する Worker 起動 bypass がない。
- ConfigBundle / Profile / provider / secret / execution binding 不足が typed diagnostic になる。
- Runtime catalog / transcript / observation / persistence initialization が Worker creation と一貫しており、永続化しない通常 Worker 作成経路がない。
- Browser-facing API に Runtime endpoint / credential / socket path / session path / raw execution handle / raw workspace path が漏れない。
- Focused tests が Worker 作成成功、duplicate launch retry、missing ConfigBundle rejection、missing execution binding rejection、execution backend connected Worker、stale execution mapping diagnostic、Companion bootstrap path を確認する。
- `cargo test -p worker-runtime --features ws-server` が通る。
- `cargo test -p yoi-workspace-server` が通る。
- `cargo check -p yoi` が通る。
- `git diff --check` が通る。
- `nix build .#yoi --no-link` が通る。
