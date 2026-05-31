# 環境変数ポリシー

INSOMNIA では、プロセス境界で本当に必要な場合を除き、環境変数の利用を避ける。新しい ambient な入力を増やすより、明示的な profile / manifest / config file / typed secret reference / CLI argument を優先する。

それでも、ユーザー設定や runtime directory の発見、package resource の発見、開発・テスト用 override、外部 provider の credential 慣習との互換のために、一部の環境変数はサポートしている。この文書に載せた環境変数は公開 surface として扱う。

## 原則

- 同じ情報を profile、manifest、config、CLI input で明示的に渡せるなら、便利ショートカットとして新しい環境変数を追加しない。
- 永続的な project state を環境変数に置かない。解決済みの状態は適切な store に保存する。
- 通常の TUI / Pod runtime は `.env` ファイルを暗黙に load しない。example が局所的に `dotenv` を使うことはあっても、application startup が project の `.env` を勝手に読むべきではない。
- 生の secret value を generated log、work item、trace output、docs に出さない。環境変数名の記述はよいが、値は書かない。
- path/location 用の環境変数、provider credential 用の環境変数、test/build-only の環境変数を混ぜない。
- test が process environment を変更する必要がある場合は、guard で直列化し、元の値を復元する。Rust の process environment は global state である。

## サポートしている runtime 環境変数

### Core config / data / resource path

これらは `manifest::paths` で解決される、application path の主要 contract である。

| 変数 | 用途 | 備考 |
| --- | --- | --- |
| `INSOMNIA_HOME` | Insomnia 管理下の config / data / runtime path の sandbox/root override。 | config は `$INSOMNIA_HOME/config`、data は `$INSOMNIA_HOME`、runtime は `$INSOMNIA_HOME/run` になる。test や isolated run で使う。 |
| `INSOMNIA_CONFIG_DIR` | 明示的な user config directory。 | 最優先の config override。`profiles.toml`、prompt override、model/provider override などを置く。 |
| `INSOMNIA_DATA_DIR` | 明示的な persistent data directory。 | 最優先の data override。session と Pod metadata を置く。 |
| `INSOMNIA_RESOURCE_DIR` | 明示的な bundled resource directory。 | 主に packaging / development の resource lookup 用。通常の installed package では `share/insomnia/resources` を使う。 |
| `XDG_CONFIG_HOME` | XDG config fallback。 | `INSOMNIA_CONFIG_DIR` と `INSOMNIA_HOME` が未設定の場合に `$XDG_CONFIG_HOME/insomnia` を使う。 |
| `HOME` | 最終的な user config / data fallback。 | `$HOME/.config/insomnia`、`$HOME/.insomnia`、`$HOME/.insomnia/run` の fallback に使う。 |

空の path 環境変数は、`manifest::paths` では原則として unset 相当に扱う。

### Runtime / socket / registry path

| 変数 | 用途 | 備考 |
| --- | --- | --- |
| `INSOMNIA_RUNTIME_DIR` | socket、pid/status file、live Pod registry file 用の明示的な runtime directory。 | 最優先の runtime override。test や isolated run でよく使う。 |
| `XDG_RUNTIME_DIR` | runtime fallback。 | `INSOMNIA_RUNTIME_DIR` と `INSOMNIA_HOME` が未設定の場合に `$XDG_RUNTIME_DIR/insomnia` を使う。 |

Runtime state は durable authority ではない。replay / restore の durable source は session log と Pod metadata であり、socket や live registry file は runtime mirror / hint である。

### Pod runtime command override

| 変数 | 用途 | 備考 |
| --- | --- | --- |
| `INSOMNIA_POD_COMMAND` | Pod runtime subprocess の起動 executable を差し替える開発・テスト用 override。 | default runtime command は現在の `insomnia` executable に `pod` prefix argument を付けたもの。この override は executable-only で、shell parse せず、`pod` prefix argument も自動付与しない。 |

これは通常の user configuration として使わない。test、development wrapper、狭い debugging 用の escape hatch である。

## Credential と外部 auth

Provider credential は、現在は manifest / profile / catalog の設定から env var 名を明示的に参照できる。これは「ambient な provider 自動発見」ではなく、設定で選んだ環境変数名を読む仕組みである。

| 変数 / pattern | 用途 | 備考 |
| --- | --- | --- |
| `INSOMNIA_API_KEY_ANTHROPIC` | custom env が指定されていない場合の Anthropic API key の default env 名。 | provider auth resolution で使う。 |
| `INSOMNIA_API_KEY_OPENAI` | OpenAI / OpenAI Responses API key の default env 名。 | provider auth resolution で使う。 |
| `INSOMNIA_API_KEY_GEMINI` | Gemini API key の default env 名。 | provider auth resolution で使う。 |
| `INSOMNIA_API_KEY_OPENROUTER` | builtin OpenRouter provider の auth hint。 | bundled provider catalog 由来。 |
| custom `model.auth.env` value | manifest / profile ごとの API key env 名。 | 明示的な config が変数名を選ぶ。`auth.env` と `auth.file` が両方ある場合は env が優先される。 |
| `BRAVE_SEARCH_API_KEY` または custom `web.search.api_key_env` | Brave WebSearch key。 | WebSearch は configured env 名だけを読み、missing / empty の場合は fail closed する。 |
| `CODEX_HOME` | Codex OAuth `auth.json` の場所。 | 外部互換用の入力。fallback は `$HOME/.codex`。 |

Credential env var は interoperability のために許容しているが、長期的に望ましい secret mechanism ではない。現時点では適切なら `auth.file` を優先し、今後は typed secret reference へ寄せる。credential UX のために implicit `.env` loading を追加しないこと。project secret を漏らしやすく、profile ごとの credential model とも相性が悪い。

## Build / example / test-only 変数

これらは通常の application configuration ではない。

| 変数 | Context | 備考 |
| --- | --- | --- |
| `CARGO_MANIFEST_DIR`, `OUT_DIR` | build / test resource lookup。 | build script や fixture/resource lookup で使う。 |
| `PATH` | test / dev command lookup。 | helper executable を探す場合だけ使う。 |
| `TMPDIR` | shell script / test。 | `tickets.sh` が temporary file に使う。 |
| `RUST_LOG` | example / dev diagnostics。 | example CLI が tracing setup 経由で読む場合がある。 |
| `ANTHROPIC_API_KEY`, `OPENAI_API_KEY`, `GEMINI_API_KEY` などの provider example vars | `llm-worker` や Pod example / fixture recorder。 | example code が `dotenv::dotenv().ok()` を呼ぶことがある。通常の `insomnia` runtime startup には適用されない。 |
| `INSOMNIA_TEST_*` | test only。 | user-facing configuration にしてはいけない。 |

## deprecated または意図的に存在しない surface

- `INSOMNIA_USER_MANIFEST` は通常の profile-based Pod/TUI startup の一部ではない。one-file manifest の debug / compatibility path には `insomnia pod --manifest <PATH>` を使う。
- ambient `.insomnia/manifest.toml` discovery は通常の fresh startup の一部ではない。
- `insomnia-pod` は installed command ではない。Pod runtime は `insomnia pod ...` から起動する。
- 通常 runtime は `.env` ファイルを load しない。

## 整理方針

環境変数に関わるコードを触る場合は、以下を優先する。

1. test env mutation を shared guard に集約し、unsafe な `set_var` / `remove_var` call を一箇所に閉じ込めて直列化する。
2. path resolution は `manifest::paths` に集約し、path precedence rule を別の場所で重複実装しない。
3. credential source は resolved config 上で明示し、process-env convention を増やすより typed secret reference へ寄せる。
4. 空の env value は、変数 category に応じて unset / invalid のどちらとして扱うかを一貫させ、新しい supported variable を追加する場合は挙動を文書化する。
5. 外部 process integration が env inheritance / filtering を必要とする場合は、ambient な inherited process state に頼らず、明示的な policy boundary として設計する。
