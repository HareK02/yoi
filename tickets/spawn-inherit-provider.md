# SpawnPod が親の provider 設定を引き継ぐ

## 背景

`SpawnPod` ツールは子 Pod に渡す overlay TOML に `pod.name` / `worker.instruction` / `scope` のみを含め、`provider` を含めない（`crates/pod/src/spawn_pod.rs::build_overlay_toml`）。

子 Pod は起動時に manifest cascade を一からやり直すため、デフォルトのインストラクション (`$insomnia/default`) を使うと `provider.kind` が見つからず `failed to resolve manifest cascade: missing required field: provider.kind` で起動失敗する。結果として `SpawnPod` ツールがデフォルト引数では事実上使えない状態になっている。

## ゴール

`SpawnPod` で起動した子 Pod が、特に指定なしで親 Pod と同じ provider 設定（kind, model, api_key 等）を使って起動できる。

## 必要な変更

- 親 Pod の resolved provider を `SpawnPodTool` から参照できるようにする（`SpawnPodTool::new` の引数追加 等）
- `build_overlay_toml` が overlay に provider を含める
- ツール入力 (`SpawnPodInput`) からの provider 指定は今回は不要（範囲外を参照）

## 完了条件

- `SpawnPod` を `provider` 指定なしで呼び出すと、子 Pod が親と同じ provider 設定で起動し、`Method::Run` を受けて応答できる
- 既存の spawn_pod 統合テスト (`crates/pod/tests/spawn_pod_test.rs`) が引き続きパス
- 親が provider を持たないケース（あり得るなら）の挙動が定まっている

## 範囲外

- ツール入力からの provider override（モデルだけ差し替えたい等のマルチエージェント用途）。将来別チケットで `SpawnPodInput` に optional provider override を追加する形で扱う
- インストラクションファイル側で provider を指定する仕組み
