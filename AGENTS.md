すでにシステムのドッグフーディングに成功しており、改善・機能追加のフェーズになっている。
随所の細かい仕様を詰めながら実装を進めている。

## このシステムに置ける設計要旨

- プロンプトはすべて resources/promptsに集約している。管理効率の向上と同時に、ユーザーがオーバーライドする形式でもある。
- E2E(実プロセスをスポーンさせてのテスト)は未設計。
- 変更量を最小にするために設計を歪めたり、設計問題に対して不必要な後方互換性を作らない。長期的なメンテナンスと型安全性を追求すること。

### LLM コンテキストの加工原則

LLM に投げる context への割り込みは、大きく2種類に分かれる。**前者は許されるが、後者は禁止**。

Podの状態から純粋に再現可能で、且つ揮発性の無い操作であることが望ましい。（pruning、tool result の content 切り詰め、prompt cache anchor の付与等）。
原則として、コンテキストは積み重ねるものであり、一時的にメッセージを差し込むことや、過去のメッセージを改ざんすることはKVキャッシュのヒット率を下げる。

**禁止**: ターンを跨ぐことができない情報に基づいて、history に記録せずに context だけにコンテンツを差し込むこと。これをやると LLM はそれに反応して生成を行う一方、次以降のターンでhistoryに残らないため、「自分がなぜその発言/tool call をしたか」の根拠が消えるうえ、prompt cache のヒット率も低下させることになる。

新しい input を context に乗せたいなら、必ず先に `worker.history` に append して commit すること。`history.json` への永続化はそこから自動的についてくる。Notify / PodEvent / `<system-reminder>` 系はこの原則で扱う。
また、キャッシュを破壊するタイミングは正確にコントロールされる必要があり、キャッシュ破壊とトークン消費のトレードオフに基づいて慎重に設計されるべきである。

---

## 実際のセッションを読んでデバッグする

`~/.yoi/sessions`にすべてのセッションがある。jsonlなので、いい感じにBashで読むこと。

---

## Git操作

明示的に指示されない限り、読み取り以外の操作は控えること。
基本はworktree上の一時的なブランチでコミットを重ね、メインブランチに取り込む運用をしている。
Orchestrator の cwd が orchestration 用ブランチ/worktree の場合、通常作業では親ブランチの dirty state を気にしない。
コミットメッセージは適当に`<prefix>: *簡潔な1行*`で書いている。

外部の参考プロジェクトは必要に応じてローカルの外部 checkout からReadすること。

---

## 検証

検証は変更内容に応じて `cargo test` / `cargo check` / `git diff --check` など、妥当な範囲で行う。重い検証は必要性が高い場合に選ぶ。

---

## Work item / Ticket の運用について

作業管理は `.yoi/tickets/` に保存される Ticket と `yoi ticket ...` コマンド / Ticket tools を正とする。時系列・状態遷移の最終的な根拠は git history なので、work item の作成・更新・レビュー・完了は Ticket 操作と commit で表現する。

### 基本コマンド

- 新規作成: `yoi ticket create --title "..." [--priority P2]`
- 一覧: `yoi ticket list [--state planning|ready|queued|inprogress|done|closed|all]`
- 詳細: `yoi ticket show <ticket-id>`
- コメント / 計画 / 判断 / 実装報告: `yoi ticket comment <ticket-id> [--role comment|plan|decision|implementation_report] [--file path]`
- レビュー記録: `yoi ticket review <ticket-id> --approve|--request-changes [--file path]`
- 状態変更: `yoi ticket state <ticket-id> planning|ready|queued|inprogress|done`
- 完了: `yoi ticket close <ticket-id> [--resolution text|--file path]`
- 整合性確認: `yoi ticket doctor`

`yoi ticket` は typed Ticket backend 経由で flat な `.yoi/tickets/<ticket-id>/` 配下の `item.md`、`thread.md`、`artifacts/` を扱う。Ticket identity はこのディレクトリ名である canonical ID のみで、title/slug words を含む alias や `open`/`pending`/`closed` bucket は現在の authority ではない。現在の lifecycle は frontmatter の `state` だけで表し、`done` と `closed` は区別する。完了時は同じ Ticket ディレクトリ内に `resolution.md` も作られる。手でファイルを作るより、原則として `yoi ticket` または Ticket tools を使うこと。

### Work item の粒度

- 1 work item = 完了時点で、実装が仕様または機能として説明できる粒度。
- 作成時は背景・要件・受け入れ条件を明確にする。実装手順やコード詳細は、必要になるまで増やしすぎない。
- チケット内の Phase / Step は実装順序であり、外部の依存関係管理として扱わない。
- ビルドが通り、その機能に限り「まだ動作できない」と明示できている場合を除き、全体として動作可能な状態を保つ。

### ライフサイクル

- 作成: `yoi ticket create ...` で `.yoi/tickets/<ticket-id>/` を作成し、必要な前提を書いて commit する。出力された canonical ID を以後の操作に使う。
- 詳細化・前提変更: `item.md` を更新し、必要に応じて `yoi ticket comment` で `thread.md` に経緯を残して commit する。
- レビュー: `yoi ticket review <ticket-id> --approve|--request-changes` で `thread.md` にレビュー結果を追記して commit する。
- 完了: `yoi ticket close <ticket-id>` で `state: closed` と `resolution.md` を同じ flat Ticket ディレクトリに記録して commit する。

worktree と併用して作業を進める場合、必ずブランチを切る前に対象 work item を作成・詳細化して commit してから切ること。

レビューは diff の確認だけでなく、work item の前提・要件・受け入れ条件が提出された実装で満たされているかを確認する。常に、その実装で良いのか、コードベースを歪めていないか、不必要な実装ではないかを確認すること。

---

YoiでYoiを開発している際、AI自身のフィードバックを元に改善を回すために `docs/report/`ディレクトリに感じた障壁や改善案等を書き残す形にした。 明確に力不足な点/ツールの問題があった場合や、ユーザーからの指示があった際に作ること。
