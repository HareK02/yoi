# External Workdir sharing permissions

`yoi workdir share` exposes a client-hosted directory through an authenticated,
outbound Backend connection. The operator selects exactly one permission level:

- **READ** (capability bits `25`): file reads and read-only discovery operations
  such as Stat, List, ViewImage, Glob, and Grep.
- **WRITE** (capability bits `31`): READ plus Write and Edit. Including READ
  preserves the read-before-write content-hash contract. WRITE never enables
  COMMAND.
- **COMMAND** (capability bits `63`): WRITE plus READ plus command start, status,
  bounded output, and cancellation. It executes on the sharing host with the OS
  authority of the user running the CLI; External Workdir command execution is
  **not a sandbox** and does not confine the process to the shared directory.

In short, `COMMAND ⇒ WRITE ⇒ READ`. READ is the default. Interactive mode asks
for one level and shows its included permissions before creating the grant. For
automation, pass `--permission` once:

```sh
yoi --backend https://example.invalid workdir share ./project \
  --workspace-id workspace-a \
  --permission command \
  --non-interactive
```

`--read-only` remains an alias for the READ level. `--read-write` remains an
alias for the WRITE level and never enables COMMAND. These legacy flags conflict
with `--permission`, and multiple `--permission` arguments are rejected so
ambiguous automation fails closed.

The grant API retains the boolean projection for compatibility, but accepts only
these combinations when creating new grants:

| Level | `read` | `write` | `command` |
|---|---:|---:|---:|
| READ | `true` | `false` | `false` |
| WRITE | `true` | `true` | `false` |
| COMMAND | `true` | `true` | `true` |

Older non-hierarchical grants such as command-only or READ+COMMAND are not
silently widened. The Backend refuses their projection, provider registration,
and subsequent restore/reconnect with guidance to revoke and re-share the
directory at one of the supported levels. General Workdir attachment/session
capability intersections may still produce partial capability sets; that
restriction applies only to External grant authority.

Omitting `--ttl` creates no automatic expiry. An explicit TTL must be between
60 seconds and 24 hours. Ctrl-C, revoke, expiry, provider disconnect, attachment
detach, and provider-generation replacement terminate and reap active commands.
A reconnect creates a fresh command namespace; old handles and results are not
accepted by the new generation.

Command output uses the existing bounded inline contract. Worker-local spill
paths are not sent to the sharing provider, and provider-local paths are never
returned to the Backend or model. Truncated External command output is therefore
reported without a full-output host path.
