# Environment variable policy

INSOMNIA should avoid environment variables unless the process boundary genuinely requires them. Prefer explicit profile/manifest fields, config files under the resolved config directory, typed secret references, or CLI arguments over adding another ambient process-global input.

Environment variables are still supported for a small number of boundaries: locating user/config/runtime directories, package resource discovery, development/runtime overrides, and compatibility with external credential conventions. Treat those variables as a public surface once documented here.

## Principles

- Do not add a new environment variable as a convenience shortcut when an explicit profile, manifest, config, or CLI input can carry the same information.
- Do not use environment variables for durable project state. Persist resolved state in the appropriate store instead.
- Do not implicitly load `.env` files in normal TUI/Pod runtime. Examples may opt into `dotenv` locally, but application startup must not silently read project `.env` files.
- Do not put raw secret values in generated logs, work items, trace output, or docs. Environment-variable names are okay to document; values are not.
- Keep path/location variables separate from provider credentials and from test/build-only variables.
- When tests must mutate process environment, serialize and restore mutations through a guard. Rust process environment is global state.

## Supported runtime variables

### Core config/data/resource paths

These are resolved by `manifest::paths` and form the main application path contract.

| Variable | Purpose | Notes |
| --- | --- | --- |
| `INSOMNIA_HOME` | Sandbox/root override for Insomnia-managed config, data, and runtime paths. | Expands to `$INSOMNIA_HOME/config`, `$INSOMNIA_HOME`, and `$INSOMNIA_HOME/run` for config/data/runtime respectively. Useful for tests and isolated runs. |
| `INSOMNIA_CONFIG_DIR` | Explicit user config directory. | Highest-priority config override. Contains `profiles.toml`, prompt overrides, model/provider overrides, etc. |
| `INSOMNIA_DATA_DIR` | Explicit persistent data directory. | Highest-priority data override. Contains sessions and Pod metadata. |
| `INSOMNIA_RESOURCE_DIR` | Explicit bundled resource directory. | Mainly for packaging/dev resource lookup. Normal installed packages use `share/insomnia/resources`. |
| `XDG_CONFIG_HOME` | XDG config fallback. | Used as `$XDG_CONFIG_HOME/insomnia` when `INSOMNIA_CONFIG_DIR` and `INSOMNIA_HOME` are unset. |
| `HOME` | Final user config/data fallback. | Used for `$HOME/.config/insomnia`, `$HOME/.insomnia`, and `$HOME/.insomnia/run` fallbacks. |

Empty path env values are generally treated as unset by `manifest::paths`.

### Runtime/socket/registry paths

| Variable | Purpose | Notes |
| --- | --- | --- |
| `INSOMNIA_RUNTIME_DIR` | Explicit runtime directory for sockets, pid/status files, and live Pod registry files. | Highest-priority runtime override. Frequently used by tests and isolated runs. |
| `XDG_RUNTIME_DIR` | Runtime fallback. | Used as `$XDG_RUNTIME_DIR/insomnia` when `INSOMNIA_RUNTIME_DIR` and `INSOMNIA_HOME` are unset. |

Runtime state is not durable authority. Session logs and Pod metadata remain the durable sources for replay/restore; sockets and live registry files are runtime mirrors/hints.

### Pod runtime command override

| Variable | Purpose | Notes |
| --- | --- | --- |
| `INSOMNIA_POD_COMMAND` | Development/test override for the executable used to launch Pod runtime subprocesses. | The default runtime command is the current `insomnia` executable plus the `pod` prefix argument. This override is executable-only: it is not shell-parsed, and no `pod` prefix arg is added. |

Do not use this as normal user configuration. It exists for tests, development wrappers, and narrow debugging cases.

## Credentials and external auth

Provider credentials may currently be referenced by env var name in manifest/profile/catalog configuration. This is an explicit reference, not ambient provider discovery.

| Variable / pattern | Purpose | Notes |
| --- | --- | --- |
| `INSOMNIA_API_KEY_ANTHROPIC` | Default Anthropic API key env name for `AuthRef::ApiKey` when no custom env is specified. | Used by provider auth resolution. |
| `INSOMNIA_API_KEY_OPENAI` | Default OpenAI/OpenAI Responses API key env name. | Used by provider auth resolution. |
| `INSOMNIA_API_KEY_GEMINI` | Default Gemini API key env name. | Used by provider auth resolution. |
| `INSOMNIA_API_KEY_OPENROUTER` | Builtin OpenRouter provider auth hint. | Comes from bundled provider catalog. |
| Custom `model.auth.env` values | Per-manifest/profile API key env name. | Explicit config chooses the variable name; env wins over `auth.file` when both are present. |
| `BRAVE_SEARCH_API_KEY` or custom `web.search.api_key_env` | Brave WebSearch key. | WebSearch reads only the configured env name and fails closed if missing/empty. |
| `CODEX_HOME` | Location of Codex OAuth `auth.json`. | External compatibility input; falls back to `$HOME/.codex`. |

Credential env vars are tolerated for interoperability, but they are not the preferred long-term secret mechanism. Prefer `auth.file` today where suitable, and typed secret references as they become implemented. Do not add implicit `.env` loading to solve credential UX; it makes project secrets too easy to leak and does not model profile-specific credentials cleanly.

## Build, examples, and test-only variables

These are not normal application configuration.

| Variable | Context | Notes |
| --- | --- | --- |
| `CARGO_MANIFEST_DIR`, `OUT_DIR` | Build/test resource lookup. | Used by build scripts and fixture/resource lookup. |
| `PATH` | Tests/dev command lookup. | Used only where locating helper executables. |
| `TMPDIR` | Shell scripts/tests. | `tickets.sh` uses it for temporary files. |
| `RUST_LOG` | Examples/dev diagnostics. | Example CLIs may honor it through tracing setup. |
| Provider example vars such as `ANTHROPIC_API_KEY`, `OPENAI_API_KEY`, `GEMINI_API_KEY` | `llm-worker` and Pod examples/fixture recorders. | Example code may call `dotenv::dotenv().ok()`. This does not apply to normal `insomnia` runtime startup. |
| `INSOMNIA_TEST_*` | Tests only. | Must not become user-facing configuration. |

## Deprecated or intentionally absent surfaces

- `INSOMNIA_USER_MANIFEST` is not part of normal profile-based Pod/TUI startup. Use `insomnia pod --manifest <PATH>` for the one-file manifest debug/compatibility path.
- Ambient `.insomnia/manifest.toml` discovery is not part of normal fresh startup.
- `insomnia-pod` is not an installed command; Pod runtime is reached through `insomnia pod ...`.
- `.env` files are not loaded by the normal runtime.

## Cleanup direction

When touching environment-variable-related code, prefer these improvements:

1. Centralize test env mutation behind a shared guard so unsafe `set_var` / `remove_var` calls stay contained and serialized.
2. Keep path resolution in `manifest::paths`; avoid duplicating path precedence rules elsewhere.
3. Make credential sources explicit in resolved config and move toward typed secret references instead of adding more process-env conventions.
4. Treat empty env values consistently as unset/invalid according to the variable's category, and document the behavior when adding a new supported variable.
5. If an external process integration needs env inheritance/filtering, design that as an explicit policy boundary rather than relying on ambient inherited process state.
