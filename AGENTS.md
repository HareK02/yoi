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

## 検証

開発中は、変更した契約を証明する最小の target / filter から実行する。

```sh
cargo test -p <crate> --lib <test-or-module-filter>
cargo test -p <crate> --test <test-target> <test-filter>
```

完了前には、workspace rootで必ず`cargo check`を実行する。rootの`cargo check`は
`default-members`に含まれるTUIやServerを含む通常のcompile closureを確認するため、公開型の
変更ごとにLLMがreverse dependencyを推測して`-p`を列挙する運用にはしない。

```sh
cargo check
cargo test -p <changed-crate>
cargo fmt --all -- --check
git diff --check HEAD
```

変更したcrate全体のtestに加え、影響するfeature構成やtest-only targetがある場合は、その検証を
追加する。`cargo check`はtestを実行せず、通常有効でないfeatureまでは確認しないため、semanticな
証明とfeature境界の検証はtargeted test/checkで補う。

workspace全体のtest、`--all-targets`、E2E、Nix/Docker buildなどの重い検証は、変更した境界を
通常のroot checkと狭い検証では証明できない場合や、明示的に要求された場合に選ぶ。実行した検証が
何を証明するのかを意識し、広い検証を形式的に回すだけにしない。

---

## ドッグフーディング時の Ticket 境界

Yoi Worker で作業する場合、Ticket の authority・ライフサイクル・操作方法は Yoi
system instructions と、その Worker に提供された typed Ticket tools に従うこと。

Codex など typed Ticket tools が提供されていないクライアントでは、`yoi ticket`
CLI や保存先の直接操作で Ticket tools を代替しないこと。Ticket の作成・更新は
Yoi Worker に委ねる。

---

YoiでYoiを開発している際、AI自身のフィードバックを元に改善を回すために `docs/report/`ディレクトリに感じた障壁や改善案等を書き残す形にした。 明確に力不足な点/ツールの問題があった場合や、ユーザーからの指示があった際に作ること。

---

絶対に自身が動作しているプロセスを止めないこと。
マージ後のドッグフーディング環境の更新は必ずユーザーの操作で行う。
