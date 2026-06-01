# Environment boundary

Environment variables are a minimized runtime boundary. Prefer explicit profile/manifest configuration and secret references over ambient process state.

## Why minimize environment variables

Ambient environment is hard to audit: it can differ between shells, services, spawned Pods, tests, and restored processes. If important runtime behavior depends on it, reproducing a session becomes harder.

Yoi keeps environment variables for narrow bootstrap and development cases, while normal provider credentials and runtime configuration should be explicit records.

## Principles

- Distinguish data/runtime paths from resource/config paths.
- Prefer embedded builtin resources over installed runtime resource directories.
- Use explicit secret refs for provider and WebSearch credentials.
- Keep dev-only executable overrides clearly named and documented.
- Avoid shell-command parser overrides for runtime Pod launch.
- Tests should prefer typed fixtures/injection and mutate process environment only around thin env-reader behavior.

## Current surface

Use `YOI_*` for current environment variables. Old project prefixes should not be reintroduced.

`YOI_POD_RUNTIME_COMMAND` is a development-only executable-path override for typed `yoi pod` launch. It is not a general shell-command override.
