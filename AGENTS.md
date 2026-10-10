すでにシステムのドッグフーディングに成功しており、ブラウザ/TUI Client/backend/runtimeの分離とチームスペースとしてのworkspaceの実装を進めている。

## このシステムに置ける設計要旨

- プロンプトはすべて`resources/prompts`に集約している。管理効率の向上のためであると同時に、ユーザーがオーバーライドする形式でもある。
- 変更量を最小にするために設計を歪めたり、設計問題に対して不必要な後方互換性を作らない。長期的なメンテナンスと型安全性を追求すること。

### 状態契約の命名・競合制御

- `revision` という名前を、大小文字や複合語を含め、新しい型・フィールド・変数・API・DB列・文書に導入しない。
- 不透明な更新番号を別の汎用名へ言い換えて済ませない。何を、どの根拠と比較し、どの操作の前／中／後に確認するかを、コードの名前と境界処理で示す。
- 到着順、操作時刻、先勝ち／後勝ちだけで成立する処理はそれで実装する。内容の一致、鍵の一致、同じ操作の再送、同じ接続の生存期間を確認する必要がある場合は、その対象を明示し、順序や時刻だけでは足りない理由を残す。
- 旧保存データを読むための綴りは現行契約と分離する。削除によるデータ喪失を避けるために残す場合も、必要な行と理由を明示して検査対象にする。文字列分割・符号化や広いディレクトリ除外で検査を逃れない。
- 完了前に `env -u LD_LIBRARY_PATH deno run --allow-read --allow-run=git tools/check-state-contract-names.ts` を実行する。例外一覧を機械的に増やして合格させない。

### LLM コンテキストの加工原則

LLM に投げる context はappend-onlyが基本であり、またその永続化形式からAPIコールの形式を純粋に再現可能である必要が有る。

一時的にメッセージを差し込む等の、揮発性の有るコンテキストの改変や、過去のメッセージを改ざんすることは基本的に禁止されている。
これを行うと、 LLM はそのコンテキストに基づいて生成を行う一方、次以降のターンでhistoryに残らないため、「自分がなぜその発言/tool call をしたか」の根拠が消えるうえ、prompt cache のヒット率も低下させることになる。

過去のコンテキストの圧縮は、キャッシュ破壊とトークン消費のトレードオフであり、必要であれば行っている。
しかし、キャッシュを破壊するタイミングと頻度は正確にコントロールされる必要があり、実際のセッションデータの解析に基づいて慎重に設計されるべきである。

---

## 検証

テストを追加・変更する前に、必ず[テスト設計規則](docs/development/rust-testing-strategy.md)を読み、その規則に従うこと。

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
