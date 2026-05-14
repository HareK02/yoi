# Invoke / Turn / LlmCall セマンティクス整理

## 背景

現在のコード・protocol・UI では `turn` / `run` / `request` の意味が混ざり始めている。

特に `llm-worker` では、`run()` によって user input を history に append し、その後 LLM 呼び出し、tool 実行、再度 LLM 呼び出し、という自走 loop が完了するまでを扱う。一方で `turn_count` や `TurnStart` / `TurnEnd` は、実態としては loop 全体ではなく、loop 内の 1 回の LLM 生成境界に近い。

界隈の慣習でも、Anthropic / OpenAI 系 API では「1 input → 1 generation」をおおむね 1 turn と呼ぶ用例が多く、外側の自走完結単位に turn を当てると読み手の期待と逆走する。

加えて `turn` は本来「順番」「誰の番か」を表す語であり、user → assistant → tool → assistant ... の各 actor 区間に当てる用途と、外側の自走完結単位に当てる用途を兼ねさせると衝突する。

今後、永続化層 (`tickets/persistence-semantics.md`) の Session / Segment 整理、compaction、fork、resume、usage accounting、TUI 表示を進めるにあたり、これらを簡潔に区別する必要がある。

## 方針

中心語を以下に整理する。

- **`Invoke`**: IDLE → active 遷移を示す **marker entry**。「ここから新しい自走サイクルが始まった」という区切りを記録するのみ。range (まとまり) は Invoke から次の Invoke までとして暗黙に表現する。kind 以外の payload は持たない。
- **`InvokeKind`**: Invoke の種別。`UserSend` / `Notify` / `PodEvent` / `SystemReminder` / `Wakeup` / ...
- **`Turn`**: 1 actor の発話・行動区間。慣習に従い、actor 視点の「番」を素直に表す。
  - `UserTurn`: user の発話
  - `AgentTurn`: assistant の応答区間 (内部に 1 または複数の `LlmCall`)
  - `ToolTurn`: tool 実行区間
  - `SystemTurn`: Hook / Notify payload / `<system-reminder>` injection など、システム介入のキャッチオール
- **`LlmCall`**: LLM を 1 回呼び、1 回の generation を得る単位。AgentTurn の内部に 1:N で属する。retry を含む。
- **`Request`**: protocol / provider / HTTP などの I/O 要求。会話上の単位には使わない。
- **`Run`**: 実装上の関数名・runtime 制御語としては残してよいが、ユーザー向け・永続化上の中心概念にはしない。

外側の自走完結単位を 1 概念として閉じ込めない (= `Exchange` / `Round` 等の新語は導入しない)。それは `Invoke` から次の `Invoke` までの range として暗黙に扱う。

### 構造

Segment 内 entry 列 (flat) として表すと以下:

```text
Invoke(kind=UserSend)            ← marker のみ (payload なし)
  UserTurn  { text }
  AgentTurn
    LlmCall { usage, content (tool_use 含む) }
    LlmCall                       ← retry (同一 input、network error / 5xx 等)
  ToolTurn  { tool_call_id, result }
  AgentTurn
    LlmCall
Invoke(kind=Notify)
  SystemTurn { notify payload }
  AgentTurn
    LlmCall
(自走中の割り込み: Invoke なし、SystemTurn のみ)
  SystemTurn { hook output }
```

ポイント:

- `Invoke` は seq に挟まる「---」相当の marker。kind のみを持ち、内容 (user の入力テキスト等) は直後の Turn entry に書く。actor 分類 (Turn) と起動 trigger (Invoke) が直交した役割を保つ。
- 自走中の割り込み (Hook 等) は IDLE → active 遷移ではないので Invoke を伴わず、SystemTurn のみが entry 列に現れる。
- TUI の "Send #N" 表示は `kind=UserSend` の Invoke を数えた連番。

### retry の境界

AgentTurn 内に LlmCall を 1:N で持たせる際、retry の境界線を以下で定義する:

- **同 AgentTurn 内の LlmCall 連続** = input messages 列が **完全に同一** な再呼び出し (network error / 5xx / rate limit / stream 切断後の再接続)
- **新 AgentTurn** = input messages 列が変化したとき (tool result が増えた、user が割り込んだ、など)

この基準は判別が明快で、usage / cost 集計も「LlmCall 単位で取り、AgentTurn 単位で sum」の 2 段集計で済む。stream 切断 → 再接続のケースが「同じ turn の継続」として自然に扱える。

### System actor の範囲

`SystemTurn` は actor 分類のキャッチオール枠として、以下を全て含む:

- Hook 出力の context 注入
- Notify payload の history 記録 (Invoke(Notify) と組で現れる)
- `<system-reminder>` injection
- pod.scope 変更などシステム由来の追記
- その他、user でも assistant でも tool でもない context 介入

これにより actor 分類は 4 値 (User / Agent / Tool / System) に収まる。

## 既存コードへの影響

### Event 名

現在の `Event::TurnStart` / `Event::TurnEnd` は実態として **LLM call 境界** で発火している。以下のいずれかで段階移行する:

- (案 A) 新規イベント `LlmCallStart` / `LlmCallEnd` を導入し、現状の TurnStart/TurnEnd の発火タイミングはそちらに移す。`TurnStart` / `TurnEnd` は AgentTurn 境界 (= retry を含むまとまり) の意味で再定義。
- (案 B) 旧 event 名は alias として残しつつ、新名 (`LlmCallStart` / `LlmCallEnd` および `AgentTurnStart` / `AgentTurnEnd` 等) で完全分離。

どちらにせよ、AgentTurn と LlmCall の発火頻度は retry 集約により異なる点だけは明示する。

### Usage / cost accounting

- Usage / prompt cache hit / provider request metrics は **LlmCall に紐づく**。
- AgentTurn の usage は内部 LlmCall の sum。
- Invoke 単位の usage が必要なら、Invoke → 次の Invoke までの全 LlmCall を sum。

### TUI

- Invoke 境界に対応するヘッダー (`Send #1` / `Notify` / `Event` を kind に応じて表示) を新設または既存 `TurnHeader` を意味繰り上げ。
- actor ごとの表示は UserTurn / AgentTurn / ToolTurn / SystemTurn それぞれに対応するブロックとして描画。
- LlmCall 境界を UI に出すかは別判断 (デフォルトは AgentTurn にまとめる)。

### Worker / Protocol

- llm-worker は IDLE → active 遷移時に Invoke marker を history に追記する責務を持つ。
- `WorkerResult` / `RunResult` / `TurnResult` などの命名は、責務 (Invoke 範囲の結果 / AgentTurn の結果 / LlmCall の結果) を区別して整理する。

## persistence-semantics との関係

`tickets/persistence-semantics.md` の Segment 内 `(segment_id, seq)` PK と整合する:

- Invoke marker / Turn 境界 / LlmCall 境界 / content entry は全て同じ entry 列に flat に並ぶ。Tree を畳む必要なし。
- fork 起点の `at_turn_index` は **Invoke marker の seq** に揃える。TUI で見る "Send #N" 境界と fork 起点が一致し、ユーザーが "N 回目の send まで戻る" と素直に認識できる。
- compaction の境界も Invoke 境界で取るのが自然 (途中 LlmCall や AgentTurn の中間では切らない)。

## 決定事項

- **Event 移行**: 案 A 採用。`Event::TurnStart` / `Event::TurnEnd` を **AgentTurn 境界** (retry 集約後) の意味に doc 再定義し、LLM call 境界の発火タイミングは **新規** `Event::LlmCallStart` / `Event::LlmCallEnd` に移す。protocol は新 variant 追加のみで既存 variant は破壊しない (互換維持)。
- **`Worker::turn_count` の意味**: **AgentTurn 数**。現状 retry が実装されていないため LLM call 数と 1:1 だが、`llm-worker-stream-continuation` 等で retry が入った時に意味が分岐する想定。
- **LlmCall 通し番号**: 新規 `llm_call_count: usize` を `Worker` に追加し、`Event::LlmCallStart` / `End` で通知。
- **`InvokeKind`**: `UserSend` / `Notify` / `PodEvent` / `SystemReminder` / `Wakeup` の 5 variants を初期セットに含める。Wakeup は実体未登場だが将来追加コストを下げるため最初から枠を切る。配置は `protocol` crate (`session-store` が依存する向き)。
- **`LogEntry::TurnEnd` の扱い**: **位置・形式そのまま維持**。新規 `LogEntry::Invoke { ts, kind: InvokeKind }` を **追加** (run / run_for_notification の開始時に commit)。TurnEnd の commit 位置を AgentTurn 単位に動かすのは互換破壊になるため、本チケット範囲外とする。`persistence-semantics` の `at_turn_index` は新 `Invoke` entry の seq を指すように後続で整合させる。
- **`SystemTurn` の sub-kind 化**: 行わない。SystemTurn は actor キャッチオール枠の単一分類として平坦に扱う。Hook / Notify / system-reminder の区別は **対応する Invoke の `kind`** で取れるため重複しない。
- **`Worker::history` への追記タイミング**: 実装中に Invoke 境界 (= run / run_for_notification 開始) と Invoke marker commit が同タイミングであることを確認する。ずれていれば marker 側を Worker 入口に揃える。

## 完了条件

- `Invoke` / `Turn` / `LlmCall` / `Request` / `Run` の定義が文書化されている。
- AgentTurn における retry の境界線が明確化されている。
- SystemTurn が actor キャッチオールとして定義され、Hook / Notify / system-reminder がそこに含まれることが示されている。
- 既存 `Event::TurnStart` / `Event::TurnEnd` の段階移行方針 (案 A) が決まっている。
- persistence-semantics の `at_turn_index` 等が Invoke seq を指すことが整合している。
- **コード反映**: 以下が実装されている。
  - `protocol::Event::InvokeStart { kind: InvokeKind }` / `Event::LlmCallStart` / `Event::LlmCallEnd` が追加されている。
  - `protocol::InvokeKind` enum が定義されている。
  - `Worker` に `llm_call_count` フィールドと `on_llm_call_start` / `on_llm_call_end` callback が追加されている。
  - `Worker::turn_count` の doc が AgentTurn 数の意味に更新されている。
  - `session_store::LogEntry::Invoke { ts, kind }` が追加され、run / run_for_notification 開始時に commit されている。
  - controller が新 callback を wire し、新 Event を broadcast している。

## 範囲外

- このチケット単体での大規模 rename 実装 (具体的には `Hook::OnTurnEnd` の名前変更、`TUI Block::TurnHeader` の名前変更、`Pod::add_on_turn_end_hook` 等の API rename)。
- `LogEntry::TurnEnd` の commit 位置を AgentTurn 単位に動かすこと (互換破壊)。
- retry の実体実装 (`llm-worker-stream-continuation` の領分)。
- 永続化 DB backend の実装。
- TUI の詳細 UX 設計 (Invoke 境界での kind 別表示、LlmCall の UI 露出など)。
- protocol の互換破壊的変更 (既存 variant の変更・削除)。

## 関連

- `tickets/persistence-semantics.md` — Segment / Entry の永続化単位を定義する別チケット。本チケットは論理単位 (Invoke / Turn / LlmCall) の定義に閉じる。
- `crates/llm-worker/`
- `crates/protocol/`
- `crates/tui/`
