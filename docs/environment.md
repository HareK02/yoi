# 環境変数ポリシー

INSOMNIA では、プロセス境界で本当に必要な場合を除き、環境変数の利用を避ける。新しい ambient な入力を増やすより、明示的な profile / manifest / config file / typed secret reference / CLI argument を優先する。

それでも、path discovery、runtime directory、外部 provider の credential 慣習との移行互換のために、一部の環境変数はまだサポートしている。この文書に載せた環境変数は公開 surface として扱う。ただし、fallback 変数は独立した設定項目ではなく、対応する main key の解決順の一部として扱う。開発・テスト都合だけの環境変数は追加しない。

## 原則

- 同じ情報を profile、manifest、config、CLI input で明示的に渡せるなら、便利ショートカットとして新しい環境変数を追加しない。
- 永続的な project state を環境変数に置かない。解決済みの状態は適切な store に保存する。
- 通常の TUI / Pod runtime は `.env` ファイルを暗黙に load しない。example が局所的に `dotenv` を使うことはあっても、application startup が project の `.env` を勝手に読むべきではない。
- 生の secret value を generated log、work item、trace output、docs に出さない。環境変数名の記述はよいが、値は書かない。
- path/location 用の環境変数、provider credential 用の移行互換、test/build-only の環境変数を混ぜない。
- test が process environment を変更する必要がある場合は、guard で直列化し、元の値を復元する。Rust の process environment は global state である。

## Path / resource discovery

Path 系の環境変数は論理的な key ごとに立項する。`XDG_*` や `HOME` は単独の設定 surface ではなく、該当 key の fallback として読む。

| 論理 key | Main env | Fallback / 解決順 | 用途と位置付け |
| --- | --- | --- | --- |
| `home` | `INSOMNIA_HOME` | なし | config / data / runtime をまとめて sandbox する root override。設定を細かく分けるより、test や isolated run ではまずこれを使う。 |
| `config_dir` | `INSOMNIA_CONFIG_DIR` | `$INSOMNIA_HOME/config` → `$XDG_CONFIG_HOME/insomnia` → `$HOME/.config/insomnia` | 人が書く設定・override の置き場。`profiles.toml`、prompt override、model/provider override など。 |
| `data_dir` | `INSOMNIA_DATA_DIR` | `$INSOMNIA_HOME` → `$HOME/.insomnia` | プログラムが書く永続データの置き場。session log、Pod metadata など、再起動後も restore / replay の根拠になるもの。通常ユーザー向けの primary knob ではなく、migration、test、isolated data store 用の advanced override。 |
| `runtime_dir` | `INSOMNIA_RUNTIME_DIR` | `$INSOMNIA_HOME/run` → `$XDG_RUNTIME_DIR/insomnia` → `$HOME/.insomnia/run` | socket、pid/status file、live registry mirror など、再起動で捨ててよい runtime state の置き場。 |

空の path 環境変数は、`manifest::paths` では原則として unset 相当に扱う。

### `data_dir` と `runtime_dir` の違い

`data_dir` は durable data のための場所である。消すと session history、Pod metadata、restore/replay の根拠が失われる。

`runtime_dir` は process coordination のための場所である。socket、pid/status file、live Pod registry mirror などを置き、プロセス終了や再起動で stale になり得る。runtime state は durable authority ではない。session log と Pod metadata が durable source であり、runtime file は mirror / hint である。

このため、socket や pid file を `data_dir` に置かない。永続データと揮発 runtime state は分ける。

### Builtin assets と `config_dir`

Builtin profiles and catalogs are embedded in the binary at build time. User/project-owned overrides remain under `config_dir` and project `.insomnia/` files such as `profiles.toml`; package runtime resource lookup is not a supported configuration surface.

## Credential と外部 auth

Provider credential は、現在は manifest / profile / catalog の設定から env var 名を明示的に参照できる。ただし、これは移行互換のための現状であり、長期的な supported configuration path ではない。`manifest-profile-encrypted-secrets` で encrypted secret store と typed secret reference を導入し、credential env var 依存は削除する方針である。

これは「ambient な provider 自動発見」ではなく、設定で選んだ環境変数名を読む仕組みである。通常 runtime が `.env` を暗黙に load することもない。

| 変数 / pattern | 用途 | 備考 |
| --- | --- | --- |
| `INSOMNIA_API_KEY_ANTHROPIC` | custom env が指定されていない場合の Anthropic API key の default env 名。 | provider auth resolution で使う。 |
| `INSOMNIA_API_KEY_OPENAI` | OpenAI / OpenAI Responses API key の default env 名。 | provider auth resolution で使う。 |
| `INSOMNIA_API_KEY_GEMINI` | Gemini API key の default env 名。 | provider auth resolution で使う。 |
| `INSOMNIA_API_KEY_OPENROUTER` | builtin OpenRouter provider の auth hint。 | bundled provider catalog 由来。 |
| custom `model.auth.env` value | manifest / profile ごとの API key env 名。 | 明示的な config が変数名を選ぶ。`auth.env` と `auth.file` が両方ある場合は env が優先される。 |
| `BRAVE_SEARCH_API_KEY` または custom `web.search.api_key_env` | Brave WebSearch key。 | WebSearch は configured env 名だけを読み、missing / empty の場合は fail closed する。 |
| `CODEX_HOME` | Codex OAuth `auth.json` の場所。 | 外部互換用の入力。fallback は `$HOME/.codex`。 |

Credential env var は interoperability のために現時点では残っているが、長期的に望ましい secret mechanism ではない。現時点では適切なら `auth.file` を優先し、今後は typed secret reference へ寄せる。credential UX のために implicit `.env` loading を追加しないこと。project secret を漏らしやすく、profile ごとの credential model とも相性が悪い。

## Build / example variables

これらは通常の application configuration ではない。

| 変数 | Context | 備考 |
| --- | --- | --- |
| `CARGO_MANIFEST_DIR`, `OUT_DIR` | build / test resource lookup。 | build script や fixture/resource lookup で使う。 |
| `PATH` | test / dev command lookup。 | helper executable を探す場合だけ使う。 |
| `TMPDIR` | shell script / test。 | `tickets.sh` が temporary file に使う。 |
| `RUST_LOG` | example / dev diagnostics。 | example CLI が tracing setup 経由で読む場合がある。 |
| `ANTHROPIC_API_KEY`, `OPENAI_API_KEY`, `GEMINI_API_KEY` などの provider example vars | `llm-worker` や Pod example / fixture recorder。 | example code が `dotenv::dotenv().ok()` を呼ぶことがある。通常の `insomnia` runtime startup には適用されない。 |

## 整理方針

環境変数に関わるコードを触る場合は、以下を優先する。

1. fallback / precedence の test は、process environment を読ませず、直接入力を渡せる小さな pure helper で検証する。
2. path resolution は `manifest::paths` に集約し、path precedence rule を別の場所で重複実装しない。
3. test が process environment を変更するのは、process env から読む thin wrapper 自体を検証する場合や、subprocess isolation に必要な場合に限る。
4. credential source は resolved config 上で明示し、process-env convention を増やすより typed secret reference へ寄せる。encrypted secret store 導入時に credential env var 依存を削除する。
5. fallback env は独立した設定項目として増やさず、対応する main key の解決順として文書化する。
6. 空の env value は、変数 category に応じて unset / invalid のどちらとして扱うかを一貫させ、新しい supported variable を追加する場合は挙動を文書化する。
7. 外部 process integration が env inheritance / filtering を必要とする場合は、ambient な inherited process state に頼らず、明示的な policy boundary として設計する。
