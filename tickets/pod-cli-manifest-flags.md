# Pod CLI: マニフェスト関連フラグの整理

## 背景

現状の `pod` CLI は `--user-manifest` / `--project` / `--overlay` の 3 つでカスケードを制御している（`crates/pod/src/main.rs`）。実運用で 2 つ足りない / 1 つ過剰:

- **`--user-manifest` フラグが過剰**: 個人マシンの user 設定は本来「一度設定して忘れる」性質のもので、毎回 CLI で渡し直す動機が薄い。XDG / `~/.config/insomnia/manifest.toml` の自動探索 + 環境変数による上書きがあれば足りる
- **環境変数連携が無い**: 「user 設定を別の場所に置きたい人」が手段を持たない
- **単一ファイル起動 mode が無い**: 「カスケードを一切回さず、このマニフェスト 1 枚だけで起動したい」という用途（再現性確保、CI、検証目的等）がカバーされていない。`--overlay` は cascade の最上位レイヤとして追加されるだけで、user / project 層は依然として読み込まれる

`--project` は workspace の起点を上書きする一般的なフラグなので（manifest 探索だけでなく将来の prompts ディレクトリ等にも効く）、そのまま残す。`--overlay` も意味的に「cascade の最上位レイヤを差し込む」で正しいのでそのまま。

## 要件

### `--user-manifest` の削除と env への置換

`--user-manifest <PATH>` フラグは削除し、代わりに `INSOMNIA_USER_MANIFEST` 環境変数で user manifest のパスを上書きできるようにする。

- env 未指定: 既存の XDG 自動探索（`manifest::user_manifest_path()`）
- env 指定: そのパスを user manifest として読む（ファイル不在はエラー、`with_user_manifest` 相当）
- env が空文字列なら未指定扱い

`pod/src/main.rs` から `user_manifest` フィールドを除き、`build_factory` 内の分岐も env 読み取りに置き換える。

### `--manifest <PATH>` の新設

カスケードを一切回さず、指定ファイル 1 枚だけを manifest として読み込んで Pod を起動する mode。

- `--project` / `--overlay` / `INSOMNIA_USER_MANIFEST` のいずれとも **併用不可**（clap の `conflicts_with` + 環境変数の事前チェック）
- 内部的には `PodFactory` の cascade 経路を通さず、`PodManifest::from_toml`（既存）の直接ルートで構築する
- prompt loader 等 cascade 経由で組まれるものは、単一ファイル mode 用に最小構成で組み直す

### 既存挙動の保持

- `--project` / `--overlay`: 変更なし
- `--adopt` / `--callback`: 変更なし（spawn 子 Pod 用、本フラグ群とは独立）
- 引数なしで `pod` を叩くと従来通り XDG → walk-up → 起動

## 範囲外

- `--no-user-manifest` / `--no-project-manifest` のような per-layer 無効化フラグ。`--manifest` で「全層無効化して 1 枚だけ」のケースはカバー出来るので、「user 層だけ無視して project 層は活かす」のような細粒度の需要が出てから別途
- `INSOMNIA_PROJECT_ROOT` 等、`--project` 側の env 連携（`--project` 自体の責務見直しと一緒に検討すべき領域なので、本チケットでは触らない）
- TUI spawn UI（`tickets/tui-pod-spawn-ui.md`）の挙動変更。spawn UI が組む overlay 経路は本チケット完了後も互換のまま動く

## 完了条件

- `--user-manifest` フラグが pod CLI から消えている
- `INSOMNIA_USER_MANIFEST=<path> pod` で user manifest のパスを上書きして起動できる
- `pod --manifest <path>` が単一ファイル mode で起動し、user / project / overlay 層は読まれない
- `--manifest` を `--project` / `--overlay` と併用しようとすると clap が拒否する
- `INSOMNIA_USER_MANIFEST` がセットされた状態で `--manifest` を使おうとするとエラーになる
- 既存の `pod --overlay <toml>` 起動（`start_pod.local.fish` 経由）が引き続き動く

## Review
- 状態: Approve with follow-up
- レビュー詳細: [./pod-cli-manifest-flags.review.md](./pod-cli-manifest-flags.review.md)
- 日付: 2026-05-12
