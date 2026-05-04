# Pod: セッションログをバックエンドにした Pod 単位の永続化

## 背景

現在の永続化の主軸は session-store の append-only JSONL ログで、`SessionId` 単位に会話履歴・設定・scope snapshot・usage・拡張 payload を復元できる。一方で Pod 単位のランタイム状態は `<runtime_dir>/{pod_name}/` 配下の `status.json` / `history.json` / `spawned_pods.json` などに write-through されているが、runtime dir は再起動で消えてよい領域であり、Pod プロセスの寿命を超える復元ソースとしては扱えない。

特に spawned Pod の管理情報は `SpawnedPodRegistry` のコメントにもある通り、現状は runtime dir への write-through のみで、再起動した spawner が子 Pod 一覧を rebuild する future work になっている。

このチケットでは、既存の session-store を物理バックエンドとして利用しつつ、Pod 名をキーにした永続状態を追加し、Pod 単位で「最後にどの session を保持していたか」「spawned children をどう復元するか」を扱えるようにする。

## 方針

- session log は引き続き会話状態の唯一の復元ソースにする。
  - `history.json` や runtime dir の snapshot を永続正本にはしない。
  - LLM context に載せる新規 input は、既存方針通り先に worker history / session log に commit されている必要がある。
- Pod 単位の永続化は「Pod identity → session / child registry などへの参照」を保存する薄いメタデータ層として設計する。
  - 会話本文を二重保存しない。
  - active session だけでなく、compaction / fork / resume によってその Pod が辿ってきた過去 session を順序付きで保持する。これは UI の履歴表示、直近以前への復元、active session 変更の監査に使う。
  - session-store の `Store` trait を拡張するか、隣接 trait / module を追加して、FsStore 以外の backend でも同じ形で実装できるようにする。
- FsStore のデフォルト layout は `<data_dir>/pods/` 配下など、`sessions/` と同じ data_dir 管理下に置く。
  - runtime dir (`<runtime_dir>/{pod_name}/`) は引き続き socket / pid / status など一時状態専用。
- Pod lifecycle 上の write point を明確にする。
  - Pod 作成時: pod name と allocated session id を記録。
  - first run で `SessionStart` が materialize された後: active session / head を更新できる状態にする。
  - compaction / fork / resume で active session が変わる場合: Pod state も同時に更新。
  - `SpawnPod` / callback / `StopPod` による child registry 変更時: runtime dir だけでなく persistent Pod state にも write-through。
- 復元時は Pod state から active session を解決し、その session log を `restore_from_manifest` 相当の経路で復元する。
  - session id を明示した resume は既存通り session を直接指定できる。
  - Pod 名 resume は Pod state → active session → session restore の順に解決する。
  - live writer 衝突は既存の pod-registry / session_id collision check を維持する。

## データ粒度の考え方

- ユーザー視点の会話継続単位と、内部の append-only log 単位を分けて扱う。
  - ユーザー視点: Pod / thread / conversation のような安定 ID。compaction しても同じ会話として継続する。
  - 内部 log 視点: session segment / revision / epoch のような履歴再構築単位。compaction や fork で新しい log root が必要なら新 ID になる。
- 現状の `SessionId` は内部 log 単位の性質が強い。compaction は履歴を要約済み prefix に置き換えて新しい append-only chain を始めるため、低レベルには「新 session」として扱うのは自然。ただし UX / データモデル上は「同じ Pod conversation の新 revision」と見せる。
- 将来 DB backend を追加する場合も、`Conversation/PodState` と `SessionSegment` を分ける形に寄せる。
  - `pod_state.active_session_id` は現在 append 先の segment を指す。
  - `pod_state.session_history[]` は Pod 視点で active だった segment の順序付き履歴。
  - compaction / fork の構造的 lineage は session log の `SessionOrigin` または DB の relation として保持し、Pod state は「この Pod がどれを active にしたか」の操作履歴に留める。

## 要件

- Pod 名をキーに、少なくとも以下を永続化できること:
  - active `SessionId`
  - ordered session history: その Pod が active として保持してきた `SessionId` の時系列リスト
    - 各 entry には最低限 `session_id` と遷移理由（new / resume / compact / fork など）を持たせる
    - compaction / fork の構造的な出自は session log の `SessionOrigin` を正本とし、Pod state 側は Pod 視点の active session 遷移履歴として扱う
  - Pod manifest / scope 復元に必要な参照または snapshot の扱い（既存 session log の `pod.scope` snapshot と責務を重複させない）
  - spawned children の registry（pod name, socket path, delegated scope, callback address, child session id が必要なら含める）
- `SpawnedPodRegistry` が runtime dir の `spawned_pods.json` だけでなく、Pod 永続状態から初期化できること。
- `ListPods` / `SendToPod` / `ReadPodOutput` / `StopPod` は、復元後の spawner でも永続化された child registry を基に動作できること。
  - ただし `ReadPodOutput` の read cursor は session-lifetime / in-memory のままでよい。永続化対象にしない。
- Pod の compaction により active session id が変わった場合、Pod 永続状態と pod-registry の session id が整合すること。
- 既存の `--session <UUID>` resume は壊さない。
- 新しい Pod 名単位 resume / attach の入口を決めること。
  - 例: `pod --pod-state <name>` ではなく、既存 `pod.name` と manifest cascade から同名 Pod state を探す形など。
  - CLI / TUI の最小導線を本チケット内で確定する。

## 完了条件

- `session-store` に Pod 単位メタデータを扱う backend API と FsStore 実装がある。
- Pod state が active session と ordered session history を保持し、new / resume / compaction / fork の遷移が順序付きで記録される。
- 新規 Pod 起動、resume、compaction、spawn / stop の各タイミングで Pod 永続状態が更新される。
- Pod プロセス再起動後、Pod 名から active session を復元し、会話を継続できる。
- spawner Pod の再起動後、永続化された spawned children 一覧から `ListPods` が復元され、到達可能な child に対して comm tools が使える。
- runtime dir は引き続き一時状態として扱われ、永続正本に依存しない。
- live writer の二重起動は既存 pod-registry / session lock と同等以上に防止される。

## 範囲外

- 会話履歴そのものの保存形式変更。
- session log の DB 化や remote backend 実装。
- Pod state の自動 GC / retention policy。
- TUI 上の高度な Pod 一覧 UI。最小限の resume / attach 導線を超える UX は別チケット。
- `ReadPodOutput` cursor の永続化。

## 関連

- `crates/session-store/`: 既存の session append-only backend。
- `crates/pod/src/runtime/dir.rs`: runtime dir の `history.json` / `spawned_pods.json`。
- `crates/pod/src/spawn/registry.rs`: spawned children registry。現状は write-through のみで復元未実装。
- `tickets/pod-session-fork.md`: active session 切り替え設計との整合が必要。
