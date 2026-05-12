# TUI spawn の user manifest env override と scope overlay の前提ズレ

## 背景

`pod` CLI は `tickets/pod-cli-manifest-flags.md` で `--user-manifest` を廃止し、`INSOMNIA_USER_MANIFEST` によって user manifest のパスを上書きできるようになった。

一方、TUI の spawn dialog は Pod 起動前に user / project manifest を先読みし、既存 cascade に `scope.allow` があるかどうかを見て、spawn overlay に cwd scope を補完するかを判断している。現状この先読みは `manifest::user_manifest_path()` に依存しており、`INSOMNIA_USER_MANIFEST` による user manifest override と一致しない可能性がある。

Pod の最終起動自体は `pod --overlay <toml>` 経由で行われるため、Pod 側では `INSOMNIA_USER_MANIFEST` が有効になる。しかし TUI が作る overlay は、別の user manifest を前提に組まれる可能性がある。

## 問題

`INSOMNIA_USER_MANIFEST` が設定されている場合、TUI spawn dialog が作成する scope overlay が実際に Pod CLI が読む manifest cascade とズレる可能性がある。

例:

- override された user manifest には `scope.allow` があるが、通常の XDG user manifest には無い
  - TUI は「cascade に scope が無い」と判断して cwd scope を overlay に追加する
  - 実際の Pod は override user manifest の scope と TUI overlay の cwd scope を両方読む
- 通常の XDG user manifest には `scope.allow` があるが、override された user manifest には無い
  - TUI は「cascade に scope がある」と判断して cwd scope を overlay に追加しない
  - 実際の Pod は override user manifest を読むため、期待より scope が狭い、または空になる可能性がある

この問題は spawn 段階で manifest が必須という意味ではなく、TUI が spawn overlay を補完するために行う cascade の事前見積もりが Pod CLI の manifest 解決規則と一致していない、という問題である。

## 要件

本チケットでは問題の記録を目的とする。対応方針はまだ決めない。

対応時には少なくとも以下を確認する:

- TUI spawn dialog が参照する user manifest 解決規則と、Pod CLI の `INSOMNIA_USER_MANIFEST` 解決規則の関係
- TUI が cwd scope を overlay に補完する条件が、実際の Pod 起動時 cascade と一致しているか
- `INSOMNIA_USER_MANIFEST` が空文字列の場合の扱い
- TUI 表示上の scope origin 表示が、実際に Pod が読む manifest と矛盾しないか

## 完了条件

- `INSOMNIA_USER_MANIFEST` 設定時に、TUI spawn dialog の scope overlay 補完が Pod CLI の実際の manifest cascade と矛盾しない
- TUI spawn dialog の表示・判断に使う user manifest 解決規則がコード上明確になっている
- 必要なテストが追加されている

## 範囲外

- TUI CLI フラグ全体のドキュメント整備
- Pod CLI の manifest flag 仕様変更
- `--project` や project manifest の env override 新設
