# TUI: 既存セッションからの Pod 復帰

## 背景

`session-store` は JSONL ログから Worker 状態を復元でき、Pod 側にも `Pod::restore(session_id, ...)` が存在する。一方で、現在の実行経路は新規 Pod 起動 (`Pod::from_manifest`) と生存中 Pod への attach / `Paused` 状態の `Resume` に限られており、停止済み Pod を既存 `SessionId` から起動するユーザー向け導線がない。

TUI には既に新規 Pod 起動用の spawn UI があるため、同じような選択 UI で既存 session を一覧し、選択した session を復元した Pod を起動して attach できるようにする。

## 要件

### 起動導線

- TUI の起動時 UI に「新規 Pod を起動」と並ぶ選択肢として「既存セッションから復帰」を用意する
- 復帰フローは現在の spawn UI と同様に alt-screen 内で完結し、選択後は復元された Pod に attach して通常のチャット画面へ遷移する
- 直接起動用に、Pod CLI に session id を指定して復元起動するフラグを追加する（例: `pod --session <UUID>`）
- TUI 復帰フローは最終的にこの Pod CLI 復元起動経路を使う

### セッション一覧

- `manifest::paths::sessions_dir()` または既存の `--store` 相当設定で解決される session store を読み、既存 session を新しい順に一覧表示する
- 一覧には少なくとも以下を表示する:
  - `SessionId`
  - 最終更新時刻、または store が提供できる同等の並び順情報
  - 履歴の簡易プレビュー（最後の user / assistant メッセージ等、取得できる範囲でよい）
- session log が壊れている、復元不能、または現在のバージョンで読めない場合は、その行を復帰不可として表示するか、エラー表示してスキップする
- session が 1 件もない場合は、新規 Pod 起動へ戻れる導線を出す

### 復元 Pod の構築

- 選択した `SessionId` を使って `Pod::restore` 経由で Pod を構築する
- manifest / scope / tool 登録 / prompt loader は、通常の新規 Pod 起動と同じ現在の cascade 解決結果を使う
- Worker の会話履歴・system prompt・request config・turn count・usage history 等は session log 由来の状態を使う
- 復元起動時、runtime の `history.json` / `status.json` / `Event::History` で TUI が初期履歴を正しく再構築できる
- 復元された session が interrupted / paused 相当の状態を持つ場合、起動直後に `Resume` 可能な状態として扱う。通常終了済みなら `Idle` として新しい入力を受け付ける

### 二重起動の扱い

- 既に生きている Pod が同じ session を持っている場合は、新規復元起動せず既存 Pod への attach を促す
- 少なくとも、同じ session id に対する複数 Pod の同時書き込みが発生しないようにする
- runtime dir / status.json から検出できる範囲でよいが、検出不能な場合のエラーメッセージは明示する

### UI / 操作

- 上下キーで session を選択し、Enter で復帰、Esc / Ctrl-C でキャンセルできる
- session が多い場合でも使えるよう、最低限のスクロールを備える
- 将来的な検索フィルタ追加を妨げない state 構造にするが、本チケットでは必須にしない
- 復帰に失敗した場合、alt-screen 内にエラーを表示し、一覧へ戻るか終了できる

## 完了条件

- `pod --session <UUID>` 相当の CLI で既存 session から Pod を起動できる
- TUI の起動導線から既存 session 一覧を表示し、選択した session を復元した Pod に attach できる
- 復元直後の TUI に過去履歴が表示される
- 復元後に新しい入力を送ると、既存履歴に続く turn として動作し、session log に追記される
- interrupted / paused 状態の session では、復元直後に Resume 導線が動作する
- 同一 session の生存中 Pod がある場合は二重起動せず attach または明示的なエラーになる

## 範囲外

- session log の全文検索 UI
- compact 前後の session chain を 1 つの論理スレッドとして束ねる UI
- 過去 session の編集・削除・名前付け
- spawn された子 Pod / scope delegation ツリー全体の復元
- 別マシンから転送された session store の import UI
