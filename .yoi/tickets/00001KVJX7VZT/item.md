---
title: 'CLI: `resume` サブコマンド化と Pod 名の暗黙解釈廃止'
state: 'inprogress'
created_at: '2026-06-20T16:18:52Z'
updated_at: '2026-06-20T16:58:30Z'
assignee: null
readiness: 'implementation_ready'
risk_flags: ['cli-ux', 'pod-metadata', 'workspace-scope', 'backward-compatibility']
queued_by: 'workspace-panel'
queued_at: '2026-06-20T16:29:26Z'
---

## User claims / request snapshot

- CLI コマンドを整理したい。
- `-r` / `--resume` は、`resume` サブコマンドにしてよいのではないか。
- top-level で subcommand になっていない文字列を打つと Pod 名として解釈される挙動をやめたい。
- `resume` は Dashboard と同様に、通常は現在 workspace 内の Pod だけを表示したい。
- `resume --all` のような明示オプションを付けたときだけ、host 上の全 Pod を一覧できるようにしたい。

## Confirmed facts / sources

- `crates/yoi/src/main.rs` の top-level parser は、既知 subcommand 以外を generic option/positional parsing に落とし、bare positional を `LaunchMode::PodName` にしている。
- 既存 parser tests には、現在の暗黙 Pod 名解釈を固定するものがある: `parse_positional_name_uses_pod_name_mode`、`parse_dashboard_word_remains_a_pod_console_name_not_an_alias`、`memory_lint_with_other_second_word_remains_positional_pod_name`。
- `yoi --help` は現在 `yoi [OPTIONS] [POD_NAME]` と `-r, --resume` を案内している。
- `crates/tui/src/lib.rs` には `LaunchMode::Resume` があり、現在は `yoi -r` / `yoi --resume` の picker 動線として扱われている。
- `crates/tui/src/console/mod.rs` の resume path は `picker::run()` を workspace 情報なしで呼んでいる。
- `crates/tui/src/picker.rs` の picker は stored Pod metadata と live Pod registry を読んで `PodList::from_sources(...)` を作っており、現状は workspace filter ではない。
- `crates/tui/src/pod_list.rs` には `PodList::from_workspace_sources(...)` があり、stored metadata の `workspace_root` が現在 workspace と一致する Pod に絞り、その Pod 名に対応する live entries だけを残す workspace-scoped list の既存部品がある。
- 関連 closed Ticket:
  - `00001KSYW63V0`: product CLI ownership / dispatch cleanup。過去には `-r`, positional Pod name, `--pod` 挙動維持が acceptance に含まれていたが、今回の依頼はその CLI UX を再整理する後続変更として扱える。
  - `00001KSXXRRC8`: LLM-facing Pod listing/restore visibility semantics。host-wide Pod enumeration を安易に広げない方針が記録されている。今回の CLI `resume --all` は human CLI 明示操作として扱う必要がある。

## Unverified hypotheses

- Dashboard の Pod 表示と完全に同一の filtering semantics にするなら、`PodList::from_workspace_sources(...)` を resume picker でも再利用するのが自然。
- `resume --all` は現在の picker 相当の host/data-dir wide behavior を明示 opt-in にする実装で足りる可能性が高い。
- CLI parser の整理は `crates/yoi/src/main.rs` と `crates/tui/src/{lib.rs,console/mod.rs,picker.rs,pod_list.rs}` の focused change で収まりそう。

## Undecided points / open questions

- この Ticket では、`-r` / `--resume` を互換 alias として残さず、`yoi resume` に寄せる方針を binding decision とする。互換 alias が必要になった場合は escalate する。
- `yoi resume --all` の表示上、host-wide であることを明示する label / warning を入れるかは実装判断でよいが、Reviewer focus に含める。

## Background

現在の top-level CLI は、bare word を Pod 名として扱うため、未知 subcommand の typo や将来追加したい command 名が Pod Console 起動に化けやすい。`resume` を明示 subcommand にし、Pod 名指定は `--pod <NAME>` など明示 path に寄せると、CLI の command surface が読みやすくなる。

## Requirements

- `yoi resume` サブコマンドを追加し、現在の `-r` / `--resume` picker 動線を移す。
- `yoi resume` は通常、現在の `--workspace <PATH>` / cwd workspace に属する Pod だけを表示する。
- `yoi resume --all` は明示 opt-in として、host/data-dir 上の全 Pod を表示できる。
- top-level の bare positional Pod name 解釈を廃止する。
  - `yoi dashboard`、`yoi memory other`、`yoi unknown` のような未知 top-level word は Pod 名として扱わず、unknown command / usage error にする。
- Pod 名を直接開く導線は、既存の明示 `--pod <NAME>` を残す。
- `yoi` 引数なしの default Console 起動は、この Ticket では変更しない。
- Help / usage / parser tests を新しい command model に合わせる。

## Acceptance criteria

- `yoi resume` が resume picker を開く。
- `yoi resume --workspace <PATH>` が指定 workspace の Pod だけを表示する。
- `yoi resume --all` が host/data-dir wide の Pod 一覧を表示する。
- `yoi -r` / `yoi --resume` は unknown/deprecated error になり、help は `yoi resume` を案内する。
- `yoi <bare-word>` は PodName mode にならず、未知 command として失敗する。
- `yoi --pod <NAME>` は明示 Pod open/attach/restore/create path として残る。
- `yoi --help` から `[POD_NAME]` の案内が消え、`yoi resume [--workspace <PATH>] [--all]` が表示される。
- Focused parser tests が、旧 positional Pod name tests を置き換え、新しい `resume` / `resume --all` / unknown command 挙動を固定する。
- Resume picker の workspace filtering について、stored metadata の `workspace_root` に基づく focused tests がある。

## Binding decisions / invariants

- Top-level bare word を Pod 名として推測しない。
- Pod 名を開くには明示 `--pod <NAME>` を使う。
- `resume --all` なしで host-wide Pod list を表示しない。
- Workspace-scoped resume は Dashboard と同じ方向の semantics を使い、Pod metadata の `workspace_root` を authority にする。
- Host-wide enumeration は human CLI の明示 `resume --all` に限定し、LLM-facing Pod tool visibility/scope とは混同しない。
- 不必要な後方互換 alias は追加しない。

## Implementation latitude

- `LaunchMode::Resume` に workspace/all の追加情報を持たせるか、別の resume options 型を作るかは実装判断でよい。
- picker API は `picker::run(options)` のように拡張してよい。
- Workspace filter 実装は `PodList::from_workspace_sources(...)` の再利用を優先するが、Dashboard と厳密に同一化するために小さな helper 抽出をしてもよい。
- Error wording / help wording は、ユーザーが `--pod` と `resume` を見つけやすい範囲で実装判断可。

## Readiness

- readiness: `implementation_ready`
- risk_flags: [`cli-ux`, `pod-metadata`, `workspace-scope`, `backward-compatibility`]

## Escalation conditions

- `workspace_root` metadata がない legacy Pod を workspace-scoped resume に含めるべきか迷う場合。
- `-r` / `--resume` を互換 alias として残す必要が出た場合。
- `resume --all` が Pod scope / permission / LLM-visible tool visibility と混ざりそうな設計になる場合。
- Parser change が `yoi pod ...`, `yoi ticket ...`, `yoi plugin ...`, `yoi memory lint ...`, `yoi panel`, `--session`, `--profile` の既存明示 path を壊しそうな場合。

## Validation

- `cargo fmt --check`
- Focused parser tests in `-p yoi`
- Focused TUI/pod-list picker tests in `-p tui`
- `cargo check -p yoi -p tui`
- `git diff --check`
- `yoi ticket doctor`
- 必要なら CLI smoke:
  - `target/debug/yoi --help`
  - `target/debug/yoi resume --help`
  - `target/debug/yoi --pod <test-name>` parser path
  - `target/debug/yoi unknown` が unknown command になること

## Related work

- Related closed Ticket: `00001KSYW63V0` — CLI ownership / dispatch cleanup
- Related closed Ticket: `00001KSXXRRC8` — Pod listing / restore visibility semantics
- Related files:
  - `crates/yoi/src/main.rs`
  - `crates/tui/src/lib.rs`
  - `crates/tui/src/console/mod.rs`
  - `crates/tui/src/picker.rs`
  - `crates/tui/src/pod_list.rs`
  - `crates/tui/src/workspace_panel.rs`
