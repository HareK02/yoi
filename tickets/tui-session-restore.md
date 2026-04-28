# TUI: 既存セッションからの Pod 復帰

## 背景

`session-store` は JSONL ログから Worker 状態を復元でき、Pod 側にも低レベルの `Pod::restore(session_id, ...)` が存在する。一方で、現在の実行経路は新規 Pod 起動 (`Pod::from_manifest`) と生存中 Pod への attach / `Paused` 状態の `Resume` に限られており、停止済み Pod を既存 `SessionId` から起動するユーザー向け導線がない。さらに既存の `Pod::restore` は manifest cascade を踏まず、`scope_lock` への登録もしない低レベル API のため、CLI / TUI からそのまま使えない。

TUI には既に新規 Pod 起動用の spawn UI があるため、同じような選択 UI で既存 session を一覧し、選択した session を復元した Pod を起動して attach できるようにする。

## 要件

### 起動導線

- TUI の既存挙動は維持する:
  - `tui`（引数なし）: 新規 Pod spawn。現在と同じ name 入力ダイアログから始める
  - `tui <pod-name>`: 生存中 Pod への attach
- 既存 session 復帰用に `tui -r` / `tui --resume` を追加する
  - `--resume` はユーザー向けの「過去 session から復帰」入口であり、protocol の `Method::Resume`（Paused turn の続行）とは別概念として扱う
  - `--resume` 指定時のみ、name 入力ダイアログの前段に session 選択プロンプトを表示する
- session id を直接指定するショートカットとして `tui --session <UUID>` を追加する
  - `--session` は session picker をスキップし、指定 session を復元対象にした name 入力ダイアログから始める
  - `--resume` と `--session` は併用不可
- Pod CLI に `pod --session <UUID>` を追加し、復元 Pod を起動できるようにする
  - 名前は他のフラグと同様 manifest cascade / overlay で決める（CLI 単体では `--overlay 'pod.name = "..."'` を使う想定）
- TUI の `--resume` / `--session` 復帰フローは、name 入力ダイアログ確定後に `pod --session <UUID> --overlay 'pod.name = "<入力名>"'` を子プロセスとして起動して attach する

### セッション一覧

- `manifest::paths::sessions_dir()` または既存の `--store` 相当設定で解決される session store を読み、新しい順に **直近 10 件** を表示する
- 一覧には少なくとも以下を表示する:
  - `SessionId`（短縮表記）
  - 並び順は `Store::list_sessions` が返す UUIDv7 順（= 作成時刻順、新しい順）でよい
  - 履歴の簡易プレビュー（最後の user / assistant メッセージ等、取得できる範囲でよい）
  - その session が今 live かどうか（`scope.lock` を引いて判定）
- session log が壊れている、復元不能、または現在のバージョンで読めない場合は、その行を復帰不可として表示するか、エラー表示してスキップする
- session が 1 件もない場合は、エラー表示して終了する（`tui` で新規 spawn する案内を出す）

### 復元 Pod の構築

復元時はソース session を直接書き継がず、**fork** して新しい session を起こす。

- `session_store::fork_at(source_session_id, source_head_hash)` で新しい session を作成する。新 session の `SessionStart` には全履歴と `forked_from = SessionOrigin { source_session_id, source_head_hash }` を載せる。
- Pod は新 session_id 上で動作し、ソース session の jsonl は不変のまま残る。
- これにより同一 session への同時書き込みは構造的に発生しない。
- manifest / scope / tool 登録 / prompt loader は、通常の新規 Pod 起動と同じ現在の cascade 解決結果を使う。
- Worker の会話履歴・system prompt・request config・turn count・usage history 等は `session_store::restore` で得た `RestoredState` を使う。`system_prompt` は session に保存された値をそのまま使い、`SystemPromptTemplate` の再レンダリングはしない。
- 復元起動時、runtime の `history.json` / `status.json` / `Event::History` で TUI が初期履歴を正しく再構築できる。
- 復元された session が interrupted / paused 相当の状態を持つ場合、起動直後に `Resume` 可能な状態として扱う。通常終了済みなら `Idle` として新しい入力を受け付ける。

### Pod / 高レベル API の整理

- `Pod::restore_from_manifest(source_session_id, manifest, store, loader) -> Pod` を新設する。中身は `from_manifest` と共通のセットアップ（pwd 解決 / scope 構築 / `scope_lock` 登録 / `provider::build_client` / `PromptCatalog::load`）を踏みつつ、上記 fork 処理と `RestoredState` 流し込みを行う。`from_manifest` / `from_manifest_spawned` / `restore_from_manifest` の共通部分は private setup 関数に括る。
- 既存の低レベル `Pod::restore(session_id, manifest, client, store, pwd, scope)` は呼出元なしのため削除する。

### `scope.lock` への session_id 追加と live 検出

`scope.lock` の `Allocation` に `session_id: SessionId` フィールドを追加し、マシン全体での「session X が今 live か」を `scope.lock` 経由で判定できるようにする。

- `register_pod` / `delegate_scope` / `adopt_allocation` / `install_top_level` のシグネチャに `session_id` を追加する。
- `LockFile::find_by_session(id)` を提供する。
- `scope_lock::lookup_session(id) -> Option<SessionLockInfo { pod_name, socket, pid }>` を提供し、`Pod::restore_from_manifest` 内で fork 前のチェックに使う。
- 検出時は `Pod::restore_from_manifest` がエラーを返し、CLI / TUI 側で適切なメッセージを出す。
- `scope.lock` のスキーマ変更については、既存ファイルが残っていたら手で消す前提（dev 期間中の互換性は不要）。

### 二重起動の扱い

- TUI の resume / `--session` 経路で `Pod::restore_from_manifest` がエラーを返した場合、エラー表示して TUI を終了する（picker 復帰や自動 attach 切替はしない）。
- ユーザーは別途 `tui <pod-name>` で attach できる。エラーメッセージには live Pod の `pod_name` / socket を含める。

### UI / 操作

- `tui -r` / `tui --resume` では、まず session picker を表示する
- session picker は上下キーで session を選択し、Enter で決定、Esc / Ctrl-C でキャンセルできる
- 直近 10 件のみ表示するためスクロール UI は不要
- session 決定後、name 入力ダイアログを表示する
  - picker と name 入力は別の inline viewport として描画してよい（高さの都合で viewport を作り直す）
  - 入力する name は、復元された session を載せる新しい Pod 実行インスタンス名（runtime dir / socket 名）
  - default name は現行と同じ cwd 由来でよい
  - 表示上は `Resume Pod` / `session: <short-id>` のように、新規 spawn ではなく復帰であることを明示する
- `tui --session <UUID>` では session picker を省略し、指定 session を対象にした name 入力ダイアログから始める
- 将来的な検索フィルタ追加を妨げない state 構造にするが、本チケットでは必須にしない
- 復帰に失敗した場合（pod プロセスが ready line を返さない、`SessionInUse` など）はエラー表示してそのまま終了する

## 完了条件

- `pod --session <UUID>` で既存 session から Pod を起動でき、ソース session jsonl は不変のまま新しい fork session が作られる
- `tui -r` / `tui --resume` で直近 10 件の既存 session 一覧を表示し、選択した session を復元対象にできる
- `tui --session <UUID>` で session picker を経由せず、指定 session の復帰 name 入力へ進める
- 復帰フローでは session 選択後または `--session` 指定後に name 入力ダイアログが表示され、その name の Pod として起動・attach できる
- 復元直後の TUI に過去履歴が表示される
- 復元後に新しい入力を送ると、既存履歴に続く turn として動作し、新しい fork session の jsonl に追記される
- interrupted / paused 状態の session では、復元直後に Resume 導線が動作する
- 同一 source session に対する live Pod が存在する場合、復元起動はエラーで終了し、既存 Pod の `pod_name` / socket がメッセージに表示される
- `scope.lock` には各 Pod の `session_id` が記録される

## 範囲外

- session log の全文検索 UI
- compact 前後の session chain / fork チェーンを 1 つの論理スレッドとして束ねる UI
- 過去 session の編集・削除・名前付け
- spawn された子 Pod / scope delegation ツリー全体の復元
- 別マシンから転送された session store の import UI
- `tui` での picker 復帰や自動 attach 切替（live セッション選択時はエラー終了）
