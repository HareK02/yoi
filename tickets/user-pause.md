# ユーザ起点の Pause と Resume

## 背景

現状 `PodStatus::Paused` に到達する経路は interceptor フック（`PreToolAction::Pause` / `TurnEndAction::Pause`）のみで、**ユーザが TUI から Paused 状態に落とす手段がない**。結果として `Method::Resume` と TUI の `Ctrl-R` は実装されているのに発火機会がなく死に筋になっている。

`Method::Cancel`（現 TUI `Ctrl-X`）はハード中止で `PodStatus::Idle` に落ちる — turn を破棄したい時には使えるが、続きから再開する手段がない。

ユーザが「今の LLM リクエストを止めて、続きは後で再開したい」「止めて、別のことを先にやりたい」という操作が可能な primitive と UI を整える。

## 依存

- `Method::Resume` / `PodStatus::Paused`: 既に実装済み、変更不要
- `cancel_tx` による worker 割り込み: 既に実装済み
- `Method::PodEvent` / `Method::Notify` の既存プロトコル拡張パターンを踏襲

## 設計

### 新しい primitive

```rust
pub enum Method {
    Run { input: String },
    Notify { message: String },
    PodEvent(PodEvent),
    Resume,
    Cancel,
    Pause,              // 新: 実行中の turn を止めて Paused 状態に落とす
    Shutdown,
    GetHistory,
}
```

**Cancel / Pause / Shutdown のセマンティクス分離**:

| Method | 実行中に受けた時 | 結果状態 | 用途 |
|---|---|---|---|
| `Cancel` | cancel_tx 発火、turn を破棄 | `Idle` | 中止して捨てる |
| `Pause` | cancel_tx 発火、turn を中断 | `Paused` | 止めるけど続きは後で |
| `Shutdown` | cancel_tx 発火、controller loop 終了 | プロセス終了 | Pod を落とす |

### Controller 側の変更

`run_with_cancel_support` に `pause_requested` フラグを導入：

```rust
async fn run_with_cancel_support<F>(...) -> (PodStatus, bool) {
    let mut shutdown_requested = false;
    let mut pause_requested = false;

    loop {
        tokio::select! {
            result = &mut pod_future => {
                return match result {
                    Ok(r) => { ... }
                    Err(PodError::Worker(WorkerError::Cancelled)) if pause_requested => {
                        // Pause 時は upward PodEvent::Errored を抑止
                        (PodStatus::Paused, shutdown_requested)
                    }
                    Err(e) => { ... PodEvent::Errored 発火 → (PodStatus::Idle, ...) }
                };
            }
            method = method_rx.recv() => {
                match method {
                    Some(Method::Cancel) => { cancel_tx.try_send(()); }
                    Some(Method::Pause) => {
                        pause_requested = true;
                        cancel_tx.try_send(());
                    }
                    Some(Method::Shutdown) => { ... }
                    ...
                }
            }
        }
    }
}
```

worker の割り込みは `cancel_tx` 一本で共通、Cancel と Pause の区別は pause フラグの有無で controller 側が判定する。

**Pause 時の upward PodEvent 抑止**: `PodEvent::Errored` は worker エラー限定の報告なので、ユーザ起点 Pause では発火させない。親 Pod への「子が Paused になった」通知は本チケットでは実装しない（必要なら別チケットで `PodEvent::Paused` variant を追加）。

**自動 turn 中の Pause**: `Method::Notify` の IDLE 自動起動、`Method::PodEvent` の auto-kick も同じ `run_with_cancel_support` を通るので Pause は同じ挙動で効く。

### 割り込みタイミングと history の整合性

`cancel_tx` 発火時の history 状態：

- **LLM ストリーム中**: block collector の Stop が発火していないので partial text / tool_call は history に入らない → history は前ラウンド末で consistent
- **Tool 実行中**: tool 自体は cancellation を知らず完了を待つ。完了後の次チェックポイントで Cancelled → history には tool_result が書かれる、orphan なし
- **tool_call 確定後 ～ tool 実行前**: orphan `tool_use` が history に残りうる
- **ラウンド間待機**: consistent、orphan なし

Resume と Pause→Run で扱いが分かれる。

### Resume の挙動

`worker.resume()` は既存実装のままで全ケース対応：

- Partial text で止まっていた場合: history は前ラウンド末なので、同じリクエストを再送 → LLM が新しく生成
- Orphan `tool_use` が残っている場合: `run_turn_loop` が `get_pending_tool_calls()` で拾って tool を実行
- 綺麗な境界で止まっていた場合: そのまま次ラウンドへ

Resume の仕様は変更なし。

### Paused → Run（新しい turn として開始）

ユーザが Paused 中に新しい入力を投げたら、**「現 turn は終わった」として新しい turn を開始**する。orphan `tool_use` がある場合は history に下記を順に追加してから `worker.run(input)` を呼ぶ：

1. 各 orphan `tool_use` に対して synthetic `tool_result`（summary: `"[Interrupted by user]"`、content なし）を inject — wire 互換性のため
2. `Item::system_message`（内容: `"[The previous turn was interrupted by the user. The user's next request follows.]"`）を inject — LLM に文脈を伝える
3. 新しい user message（`Run` の input）を append
4. `run_turn_loop` に入る

実装する API の目安：`Pod::interrupt_and_run(input: String)` みたいな名前で上記 4 ステップを提供。Controller が Paused → Run 遷移時に呼ぶ。

注: 実機テストで Anthropic が orphan `tool_use`（次 user turn に `tool_result` が含まれない形）を許容するようなら step 1 は削除できる。まずは安全側に倒して両方入れる。

### Idle / Paused 時の Pause / Cancel

- `Pause` when `Idle`: `Event::Error { NotRunning }` を返す（`Cancel` と同じ）
- `Pause` when `Paused`: no-op（既に Paused）
- `Cancel` when `Idle` / `Paused`: `NotRunning` エラー（既存の Cancel 挙動と揃える）

### TUI のキー割り当て

現状：`Ctrl-X` = Cancel、`Ctrl-R` = Resume、`Ctrl-D` = Shutdown（running なら 2 連打確認）、`Esc` = TUI 終了、`Ctrl-C` = TUI 終了。

変更後：

| キー | Running 時 | Idle / Paused 時 |
|---|---|---|
| `Ctrl-X` | `Method::Cancel`（破棄 → Idle） | no-op（エラー表示） |
| `Ctrl-C` | `Method::Pause`（→ Paused） | 1 回目 warn / 3 秒以内の 2 回目で TUI 終了（Pod は残る） |
| `Ctrl-D` | 1 回目 warn / 3 秒以内の 2 回目で `Method::Shutdown` | `Method::Shutdown`（Pod を落とす） |
| `Enter` (入力あり) | —（入力は TUI 側でバッファ） | Idle なら `Method::Run`。Paused なら同じく `Method::Run`（Controller が前 turn を打ち切って新 turn 開始） |
| `Enter` (空) | — | Paused なら `Method::Resume`。Idle なら no-op |

`Ctrl-R` と `Esc` は廃止。

Ctrl-C の 2 段階 UX は Ctrl-D と対称：running 中の 1 回目は安全な中断（Pause）、非 running 時の 1 回目は確認 warn、どちらも連打で最終アクション。

### Paused 状態の TUI 表示

現状 `App` は `running: bool` のみで Paused を区別していない。`ui.rs:draw_status` も「running / idle」の 2 状態のみ。

追加：

- `App` に `paused: bool`（または `status: enum { Idle, Running, Paused }`）
- `Event::RunEnd { result: RunResult::Paused }` で `paused = true`、`running = false`
- `Event::TurnStart` または新しい Run / Resume 送信時に clear
- `draw_status` に paused 分岐を追加、hint は `Enter to resume, type to start new turn`

## 影響範囲

- `crates/protocol/src/lib.rs`: `Method::Pause` variant 追加、serde round-trip テスト
- `crates/pod/src/controller.rs`: `run_with_cancel_support` に `pause_requested` 追加、Paused → Run の interrupt 処理
- `crates/pod/src/pod.rs`: `interrupt_and_run(input)` 相当の API（orphan 閉じ + system note inject + run）
- `crates/tui/src/app.rs`: `paused` フラグ、`submit_input` の Paused 分岐（空なら Resume）
- `crates/tui/src/main.rs`: `handle_key` のキー割り当て変更（Ctrl-X / Ctrl-C / Ctrl-D）、Ctrl-C の 2 連打 Quit、`Ctrl-R` / `Esc` 削除
- `crates/tui/src/ui.rs`: `draw_status` の paused 分岐
- `docs/tui-keybindings.md`: 既存ファイルを更新
- 既存の interceptor-driven Pause（`PreToolAction::Pause` / `TurnEndAction::Pause`）は挙動不変

## 完了条件

- `Method::Pause` が protocol crate に追加され、serde round-trip テストが通る
- 実行中 Pod に `Method::Pause` を送ると `PodStatus::Paused` に落ち、`Method::Resume` で続きから再開できる
- TUI で `Ctrl-C` を押すと Pause される（LLM ストリーム中・tool 実行中どちらからでも、Notify / PodEvent 起点の自動 turn 中でも）
- Pause 時に親 Pod への `PodEvent::Errored` が飛ばない（upward 通知抑止）
- Paused 中に空 Enter → Resume、入力あり Enter → 新 turn として Run（orphan tool_use は synthetic tool_result + system note で閉じる）
- Paused → Run 後、LLM への送信が wire 上正しい（orphan tool_use が解消されている）ことを統合テストで確認
- Pause → Resume の history consistency を統合テストで確認
- `Ctrl-X` = Cancel（破棄 → Idle）、`Ctrl-C` = Pause / 2 連打で TUI 終了、`Ctrl-D` = Shutdown（running 中は 2 連打）
- `Ctrl-R` / `Esc` キーが無効化されている
- TUI status line に Paused 状態と Enter ヒントが表示される
- 既存の Cancel / Shutdown / interceptor-driven Pause の挙動が回帰しない

## 範囲外

- 複数クライアントが同時に Pod を触る時の race。最後の勝ち or 冪等挙動で十分
- Tool 実行の cancellation 対応（長時間 tool を interrupt する仕組み）
- 親 Pod への `PodEvent::Paused` 通知
- Anthropic が orphan `tool_use` を本当に 400 で弾くかの実機検証。まず synthetic tool_result を入れて安全側で実装、許容されるなら後で削る
