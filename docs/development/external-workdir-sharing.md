# External Workdir sharing permissions

`yoi workdir share` exposes a client-hosted directory through an authenticated,
outbound Backend connection. The operator grants three independent categories:

- **READ**: file reads and read-only discovery operations such as Stat, List,
  ViewImage, Glob, and Grep.
- **WRITE**: Write and Edit. WRITE requires READ because updating an existing
  file uses the read-before-write content-hash contract. Selecting WRITE never
  enables COMMAND.
- **COMMAND**: command start, status, bounded output, and cancellation. COMMAND
  does not enable READ or WRITE. It executes on the sharing host with the OS
  authority of the user running the CLI; External Workdir command execution is
  **not a sandbox** and does not confine the process to the shared directory.

READ is enabled by default and COMMAND is disabled by default. Interactive mode
asks for a comma-separated category list and shows the exact grant before it is
created. For automation, repeat `--permission`:

```sh
yoi --backend https://example.invalid workdir share ./project \
  --workspace-id workspace-a \
  --permission read \
  --permission write \
  --permission command \
  --non-interactive
```

`--read-only` remains an alias for READ. `--read-write` remains an alias for
READ+WRITE and never enables COMMAND. These legacy flags conflict with explicit
`--permission` arguments so ambiguous automation fails closed.

Omitting `--ttl` creates no automatic expiry. An explicit TTL must be between
60 seconds and 24 hours. Ctrl-C, revoke, expiry, provider disconnect, attachment
detach, and provider-generation replacement terminate and reap active commands.
A reconnect creates a fresh command namespace; old handles and results are not
accepted by the new generation.

Command output uses the existing bounded inline contract. Worker-local spill
paths are not sent to the sharing provider, and provider-local paths are never
returned to the Backend or model. Truncated External command output is therefore
reported without a full-output host path.
