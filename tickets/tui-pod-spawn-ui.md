# TUI: inline viewport で Pod を spawn する UX

## 背景

現在の Pod 起動はシェル直叩きで、`pod` バイナリに manifest を渡して即フルスクリーン TUI に入る形になっている (`start_pod.local.fish` 参照)。ユーザーの自然なメンタルモデルは「ワークスペースに `cd` して、最上位エージェントを 1 個立ち上げ、そのセッションでオーケストレーションを進める」というもので、現状はこの「立ち上げる」操作の前段が貧弱。

複数 Pod の並列実行は別ターミナル / tmux で別プロセスとして並べる運用を想定するため、TUI 自体を multi-pod 化する方向には踏み込まない。**1 シェル起動 = 1 Pod に attach** の前提を維持したまま、起動の前段だけ対話的にする。

manifest カスケード (`crates/pod/src/factory.rs`) の前提:

- **user manifest** (`~/.config/insomnia/manifest.toml`): `model` と `auth` 等、ユーザー横断で固定したい設定の置き場。spawn UI はここに `model` があることを前提にする
- **project manifest** (`<workspace>/.insomnia/manifest.toml`): あれば `pod.name` / `scope.allow` を上書きする
- **overlay**: spawn UI が dialog 入力からその場で組み立てて pod に渡す最高優先度のレイヤ

spawn UI の役割は、project manifest が無いワークスペースでも overlay を組んで起動できるようにし、`.insomnia/manifest.toml` を作る手間を省くこと。

## 方針

- 起動コマンドはまず **inline viewport** でダイアログを描く（ratatui の `Viewport::Inline`）
- ダイアログで manifest と起動パラメータを確定 → Pod を spawn → そのまま **fullscreen TUI** (alternate screen buffer, `tickets/tui-fullscreen-overhaul.md`) に attach する
- ダイアログのやり取りはシェルのスクロールバックに残す。「何を spawn したか」がログとして自然に残るのがこの方式の主眼
- キャンセル経路では fullscreen に入らずシェルに戻る

`tui-fullscreen-overhaul.md` は attach 後の本体描画モデル、本チケットはその**前段の対話 UI と attach までの遷移**を扱う。両者は viewport の使い分け（inline → alternate screen）で繋がる。

## 要件

### エントリポイント

- ワークスペース直下で叩いて Pod を立ち上げるコマンドを 1 本提供する（既存 `pod` バイナリの起動経路を流用するか別サブコマンドにするかは設計で決める）
- 引数なしで叩いた場合は inline ダイアログに入る
- manifest を引数で渡された場合はダイアログをスキップして直接 fullscreen に入る（既存の動作を保つ）

### inline ダイアログ

- ratatui の inline viewport で、シェル直下の数行に描く
- 入力項目:
  - **pod.name** (1 行テキスト、編集可): デフォルト値は project manifest の `pod.name`、無ければ cwd の basename
  - その他の必要設定（model、scope.allow）はカスケード or デフォルトで補う（後述）
- 確定 / キャンセルが明示的なキー操作で、誤爆しないこと
- 確定すると Pod 起動が始まる。起動中の進捗を inline 領域に流して、attach 完了の瞬間に fullscreen へ切り替える
- キャンセル時は inline 領域を畳んでシェルに戻り、Pod は spawn しない

### 必須フィールドの埋め方

pod は `pod.name` / `model` / `scope.allow` がそろわないと起動しない。spawn UI はそれぞれを次のように埋める:

- **`pod.name`**: ダイアログ入力。未入力で Enter は不可。デフォルト値は user / project manifest に既に書かれていればそれ、無ければ cwd の basename
- **`model`**: user / project どちらかのレイヤから cascade 経由で取得。どこにも無ければ pod 側の resolve が失敗するので、その stderr エラー文を inline ダイアログに表示してキャンセル相当に倒す（user manifest の編集 UI までは本チケット外）
- **`scope.allow`**: user / project どちらかのレイヤに既にあればそれをそのまま使う（overlay には追加しない）。両方とも無ければ `target = <cwd>, permission = "write"` をデフォルトとして overlay に追加する

tui は `manifest` クレートの `PodManifestConfig::from_toml` / `merge` を使って user + project の cascade を実際にマージし、その結果から「`scope.allow` が空かどうか」を読み取る。実際の最終マージ + バリデーションは pod の `PodFactory::resolve()` 側で行われるので、tui は dialog 用の事前情報を取るためだけに同じ仕組みを再実行する形になる（重い `pod` クレート全体には依存しない）。

### `manifest` クレートへのカスケード収集 API 移管

カスケードのファイルシステム規約（user manifest の XDG パス、project manifest の `.insomnia/manifest.toml` 上方探索、TOML 読み込み + パース）は、当初 pod の `factory.rs` 内 private に実装されていた。本チケットで tui が同じ規約を必要としたため、二箇所に重複させるのではなく **`manifest` クレートに公開 API として移管**する。

- `manifest::user_manifest_path() -> Option<PathBuf>`
- `manifest::find_project_manifest_from(start: &Path) -> Option<PathBuf>`
- `manifest::load_layer(path: &Path) -> Result<PodManifestConfig, LayerLoadError>`

pod の `PodFactory` も tui の spawn UI もこれらを呼ぶ形に統一し、規約は manifest に一箇所で持つ。`PodFactory` 自体（builder + PromptLoader 抱える型）は引き続き pod の責務として残す。

確定時、ダイアログ入力 + デフォルト埋めから overlay TOML を組み、pod に `--overlay` で渡す。pod 側の cascade は user → project → overlay の順で merge されるので、project manifest に値があれば overlay の同名フィールドだけが上書きする形になる。

### inline → fullscreen の遷移

- inline viewport を畳む → alternate screen に切り替える、を**ちらつかず**に実行する
- 切り替え後、Pod の `Event::History` で履歴を組み直して通常の TUI 状態に入る（ここから先は fullscreen overhaul の責務）
- 切り替え失敗（Pod 起動失敗等）の場合は inline 領域にエラーを出して終了する。alternate screen には入らない

### スクロールバックに残るもの

- inline ダイアログで確定した spawn の要約（manifest path、override の要点）
- 起動失敗時のエラー
- 正常 attach した場合の「attach 開始」ログ 1 行程度

fullscreen TUI で描いた内容は alternate screen buffer なのでスクロールバックには残らない（既存方針通り）。

## 設計で決めること

- **`pod.name` の文字種制約**: runtime dir 名に使われるのでファイルシステム安全な範囲に絞る（英数 + `-` + `_` + `.` 等）
- **scope デフォルトの permission**: `write` で良いか、対話的に `read` / `write` を切り替えさせるか
- **キーバインド**: 確定 / キャンセル / 項目移動。fullscreen 側のキーマップと衝突しないこと
- **進捗表示の粒度**: Pod 側の起動シーケンスのどのフェーズを inline に出すか
- **再 attach の入り口**: 既存 Pod に後から attach するユースケースを今回扱うか、扱うなら inline ダイアログの中に「新規 spawn / 既存 attach」の分岐を置くか別コマンドに分けるか
- **user manifest 不在時の扱い**: 「先に `~/.config/insomnia/manifest.toml` を作ってください」とエラーするか、user manifest 編集 UI までこのチケットで踏み込むか

## 完了条件

- ワークスペースで該当コマンドを引数なしで叩くと inline ダイアログが立ち上がる
- ダイアログで `pod.name` を確定すると、必要なら scope.allow デフォルトを埋めた overlay が組まれて Pod が spawn され、そのまま fullscreen TUI に attach する
- project manifest が無いワークスペースでも、user manifest に model があれば spawn できる
- user manifest に model が無いと、ダイアログ内で何が足りないかが分かるエラーが出る
- ダイアログでキャンセルするとシェルに戻り、Pod は起動していない
- manifest を引数で直接渡した（または既存 Pod 名で attach した）場合はダイアログを経由せず従来通り fullscreen に入る
- 確定した spawn の要約と、起動失敗時のエラーがスクロールバックに残る
- inline → fullscreen の遷移でターミナル表示が破綻しない

## 範囲外

- TUI の中で複数 Pod を tab / split / list で切り替える UI
- Pod 間メッセージパッシング、依存関係
- 既存 Pod への再 attach（扱うかは「設計で決めること」で判断、扱わないと決まれば別チケット）
- Pod テンプレートの管理 UI（保存・編集・共有）
- リモート Pod / 分散実行
