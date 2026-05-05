# Exchange / Turn / Call セマンティクス整理

## 背景

現在のコード・protocol・UI では `turn` / `run` / `request` の意味が混ざり始めている。

特に `llm-worker` では、`run()` によって user input を history に append し、その後 LLM 呼び出し、tool 実行、再度 LLM 呼び出し、という自走 loop が完了するまでを扱う。一方で `turn_count` や `TurnStart` / `TurnEnd` は、実態としては loop 全体ではなく、loop 内の 1 回の LLM 生成境界に近い。

ただし `Turn` は本来「誰の番か」「誰が発話・行動する区間か」を表す語であり、user input から Agent の自走完了までの外側のまとまりを `Turn` と呼ぶと語感が歪む。

今後、永続化層の `Thread` / `Segment` 整理、compaction、fork、resume、usage accounting、TUI 表示を進めるにあたり、外側のまとまり、actor ごとの turn、LLM 呼び出し、I/O request を簡潔に区別する必要がある。

## 方針

このプロジェクトでは、中心語を以下のように整理する。

- `Exchange`: history に commit された 1 つの外部 input を起点に、Agent が LLM 呼び出しや tool 実行を含めて自走し、いったん停止状態に戻るまでのまとまり。
- `Turn`: Exchange 内で、ある actor が発話・行動する区間。例: user turn / assistant turn / tool turn / system turn。
- `Call` / `LlmCall`: LLM を 1 回呼び、1 回の assistant generation を得る単位。assistant turn を実現する低レイヤー単位だが、retry / continuation などを考えると turn と同義にはしない。
- `Request`: protocol / provider / HTTP などの I/O 要求。会話上の単位としては使わない。
- `Run`: 実装上の関数名・runtime 制御語としては残してよいが、ユーザー向け・永続化上の中心概念にはしない。

構造としては以下を基本にする。

```text
Thread
  └─ Segment
      └─ Exchange
          ├─ UserTurn
          ├─ AssistantTurn
          │   └─ LlmCall
          ├─ ToolTurn
          │   └─ ToolExecution
          ├─ AssistantTurn
          │   └─ LlmCall
          └─ ...
```

## 要件

- `Exchange` / `Turn` / `Call` / `Request` / `Run` の定義を、永続化セマンティクスおよび worker/protocol/UI の用語に反映できる状態にする。
- 裸の `turn` は actor ごとの発話・行動区間を指す語として扱い、外側の自走完了単位には使わない。
- 現在 LLM 1 回生成を `turn` と呼んでいる箇所は、原則として `call` / `llm_call` へ寄せる方針を明示する。
- ユーザー向け表示では、外側のまとまりを `Exchange` またはそれに相当する表示概念として扱う。`Turn` と表示する場合は actor-based turn との混同を避ける。
- Usage / prompt cache / provider request metrics は `LlmCall` に紐づくものとして整理する。
- `Request` は I/O request に限定し、Exchange / Turn と同義にしない。
- 既存 protocol 互換が必要な場合、`TurnStart` / `TurnEnd` などは段階移行または alias を検討する。

## 検討事項

- 現在の `Event::TurnStart` / `Event::TurnEnd` が実態として LLM Call 境界であることをどう移行するか。
  - 例: `LlmCallStart` / `LlmCallEnd` を追加し、既存イベントは互換用 alias とする。
- TUI の `TurnHeader` が何を表すべきか。
  - 外側のまとまりなら `ExchangeHeader` 相当に寄せる。
  - actor ごとの区間なら `UserTurn` / `AssistantTurn` / `ToolTurn` の表示に寄せる。
  - LLM Call ごとの表示を残す場合は `CallHeader` 相当に名称を変える。
- `WorkerResult` / `RunResult` / `TurnResult` の責務境界。
  - Exchange の結果、actor Turn の結果、LlmCall の結果を混同しない。
- compaction / resume により Exchange が複数 Segment または複数 runtime attempt にまたがる場合の扱い。
- `persistence-semantics.md` の `Thread` / `Segment` / `Entry` / `Checkpoint` モデルへの反映。
- `Notify` / `PodEvent` / `SystemReminder` など user input ではない起点を Exchange と呼ぶことが適切か、または `ExchangeKind` のような分類で表すか。

## 完了条件

- `Exchange` / `Turn` / `Call` / `Request` / `Run` の定義が文書化されている。
- `persistence-semantics.md` に反映すべき前提が整理されている。
- worker / protocol / TUI / usage accounting で rename または互換 alias が必要な箇所が洗い出されている。
- 実装変更を行う場合、外側 Exchange、actor Turn、内側 LlmCall の境界がコード上で判別できる。
- 既存 API / event 名をすぐ壊さない段階移行方針が決まっている。

## 範囲外

- このチケット単体での大規模 rename 実装。
- 永続化 DB backend の実装。
- TUI の詳細 UX 設計。
- protocol の互換破壊的変更。

## 関連

- `tickets/persistence-semantics.md`
- `tickets/pod-persistent-state.md`
- `tickets/pod-session-fork.md`
- `crates/llm-worker/`
- `crates/protocol/`
- `crates/tui/`
