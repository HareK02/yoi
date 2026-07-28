すでにシステムのドッグフーディングに成功しているが、一旦安定した旧バージョンで、ブラウザ/TUI Client/backend/runtimeの分離とチームスペースとしてのworkspaceを作るObjectiveを進めている。

## このシステムに置ける設計要旨

- プロンプトはすべて resources/promptsに集約している。管理効率の向上と同時に、ユーザーがオーバーライドする形式でもある。
- 変更量を最小にするために設計を歪めたり、設計問題に対して不必要な後方互換性を作らない。長期的なメンテナンスと型安全性を追求すること。

### LLM コンテキストの加工原則

LLM に投げる context への割り込みは、大きく2種類に分かれる。**前者は許されるが、後者は禁止**。

Workerの状態から純粋に再現可能で、且つ揮発性の無い操作であることが望ましい。（pruning、tool result の content 切り詰め、prompt cache anchor の付与等）。
原則として、コンテキストは積み重ねるものであり、一時的にメッセージを差し込むことや、過去のメッセージを改ざんすることはKVキャッシュのヒット率を下げる。

**禁止**: ターンを跨ぐことができない情報に基づいて、history に記録せずに context だけにコンテンツを差し込むこと。これをやると LLM はそれに反応して生成を行う一方、次以降のターンでhistoryに残らないため、「自分がなぜその発言/tool call をしたか」の根拠が消えるうえ、prompt cache のヒット率も低下させることになる。

新しい input を context に乗せたいなら、必ず先に `worker.history` に append して commit すること。`history.json` への永続化はそこから自動的についてくる。Notify / WorkerEvent / `<system-reminder>` 系はこの原則で扱う。
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

## ドッグフーディング時の Ticket 境界

Yoi Worker で作業する場合、Ticket の authority・ライフサイクル・操作方法は Yoi
system instructions と、その Worker に提供された typed Ticket tools に従うこと。
backend や CLI の具体的な手順はこのリポジトリの `AGENTS.md` では重複して定義しない。

Codex など typed Ticket tools が提供されていないクライアントでは、`yoi ticket`
CLI や保存先の直接操作で Ticket tools を代替しないこと。Ticket の作成・更新は
Yoi Worker に委ねる。

---

YoiでYoiを開発している際、AI自身のフィードバックを元に改善を回すために `docs/report/`ディレクトリに感じた障壁や改善案等を書き残す形にした。 明確に力不足な点/ツールの問題があった場合や、ユーザーからの指示があった際に作ること。
