# Server / Runtime manual auth setup

Workspace Server and Worker Runtime authenticate remote Runtime control traffic with manually exchanged Ed25519 public keys and short-lived Server-signed capability tokens.

This is a non-interactive bootstrap flow. Commands fail when required flags are missing, and existing identity/trust records are not overwritten unless `--replace` is passed explicitly.

## Authority boundary

- Workspace Server is the workspace control plane. It owns trusted Runtime records in the Server DB and signs per-request Runtime capability tokens.
- Runtime owns Worker execution. It does not own a workspace registry or workspace list.
- Runtime API paths remain worker-centric; workspace scope is carried in the signed auth context and enforced by Runtime-side authorization/filtering code.
- Browser/Web clients should talk to Workspace Server, not directly to Runtime.

## Identifiers used in examples

Replace these values for the deployment:

```text
SERVER_ID=server-main
RUNTIME_ID=runtime-main
RUNTIME_BASE_URL=http://127.0.0.1:38800
```

`SERVER_ID` is the issuer id in Server-signed tokens. `RUNTIME_ID` is the token audience and must match the Runtime identity.

## 1. Create and show the Server identity

From the Workspace Server host:

```bash
yoi-server identity init --server-id server-main
```

Show the public identity and copy the `public_key` value:

```bash
yoi-server identity show --json
```

The Server private identity is stored in the Yoi data directory under the Server data root, currently:

```text
<data_dir>/server/identity.toml
```

On Unix this file is written with `0600` permissions. Do not copy the private key to Runtime or commit it to the repository.

## 2. Create and show the Runtime identity

From the Runtime host, using the same Runtime storage flags that the Runtime server process will use:

```bash
yoi-runtime identity init --runtime-id runtime-main
```

Show the public identity and copy the `public_key` value:

```bash
yoi-runtime identity show --json
```

By default, Runtime auth state is stored at:

```text
<data_dir>/runtime/auth.toml
```

If the Runtime process is launched with `--fs-root` or `--fs-runtime-dir`, pass the same flags to every `identity`, `trust-server`, and `trust-workspace` command. Otherwise the setup command may write an auth file that the server process never reads.

Example with explicit Runtime storage:

```bash
yoi-runtime identity init \
  --runtime-id runtime-main \
  --fs-root /var/lib/yoi-runtime

yoi-runtime identity show \
  --json \
  --fs-root /var/lib/yoi-runtime
```

## 3. Register the Server public key on Runtime

On the Runtime host, register the Server public key copied from `yoi-server identity show --json`:

```bash
yoi-runtime trust-server add \
  --server-id server-main \
  --public-key '<SERVER_PUBLIC_KEY>'
```

With explicit Runtime storage, keep using the same storage flags:

```bash
yoi-runtime trust-server add \
  --server-id server-main \
  --public-key '<SERVER_PUBLIC_KEY>' \
  --fs-root /var/lib/yoi-runtime
```

Verify:

```bash
yoi-runtime trust-server list --json
```

## Workspace issuer trust foundation

Workspace signing identity is Workspace-scoped. Provision the identity through the authenticated owner-only Workspace Settings operation, then export the public bundle from:

```text
GET /api/w/<WORKSPACE_ID>/settings/workspace/signing-identity
```

Save the response's non-null `public_bundle` object—not the outer response wrapper—as `workspace-public-identity.json`, and transfer only that document to the Runtime host:

```bash
yoi-runtime trust-workspace add \
  --bundle workspace-public-identity.json \
  --fs-root /var/lib/yoi-runtime
```

The bundle contains `workspace_id`, `backend_url`, `key_id`, `algorithm`, `public_key`, `public_key_fingerprint`, and the Workspace signing identity `revision`. It never contains the Workspace private key. Do not transfer the Server-side private-material file or place private material in Runtime configuration.

Inspect the Runtime trust records without exposing private material:

```bash
yoi-runtime trust-workspace list --fs-root /var/lib/yoi-runtime
yoi-runtime trust-workspace show \
  --workspace-id '<WORKSPACE_ID>' \
  --fs-root /var/lib/yoi-runtime
```

An exact repeated `add` is idempotent. A different bundle for an existing Workspace is rejected; use the explicit `replace` operation after verifying the new public fingerprint out of band:

```bash
yoi-runtime trust-workspace replace \
  --bundle workspace-public-identity-v2.json \
  --fs-root /var/lib/yoi-runtime
```

Runtime increments a local `trust_generation` on replacement and revocation. Signed claims bind the issuer URL, Workspace signing identity revision, Runtime trust generation, live Workspace–Runtime `binding_revision`, Runtime/Worker target, operation, request-body SHA-256 digest, expiry, and one-time `jti`. Claims are accepted only when the Workspace identity revision, Runtime trust generation, and caller-supplied current binding revision all match exactly. Revoke trust without deleting its generation fence:

```bash
yoi-runtime trust-workspace revoke \
  --workspace-id '<WORKSPACE_ID>' \
  --fs-root /var/lib/yoi-runtime
```

For rotation, create and export the new Workspace identity first, verify its fingerprint, replace Runtime trust, update the Workspace–Runtime binding authority, and only then issue claims under the new key/generation. For emergency revocation, revoke Runtime trust first and stop issuing claims; reactivation requires an explicit `replace` with an active public bundle. Runtime auth state is written atomically with private file permissions and survives restart; malformed trust state fails closed.

This command establishes the Runtime-side trust and claim-verifier foundation only. The Runtime auth store is read during process startup; once signed verification is connected, a controlled Runtime restart will be required before a changed trust record affects verification. Do not restart a live Runtime until its active Worker lifecycle has been handled. Until the signed-verification cutover Ticket is integrated, existing remote control traffic continues to select the legacy trusted-Server verifier explicitly; Runtime never falls back from one verifier mode to the other.

## 4. Register the Runtime public key and endpoint on Server

On the Workspace Server host, register the Runtime public key copied from `yoi-runtime identity show --json`:

```bash
yoi-server trust-runtime add \
  --workspace-id '<WORKSPACE_ID>' \
  --runtime-id runtime-main \
  --base-url http://127.0.0.1:38800 \
  --public-key '<RUNTIME_PUBLIC_KEY>' \
  --display-name 'Runtime main'
```

This writes a Workspace-scoped Runtime binding and trust fingerprint to the Server DB. During `yoi-server serve`, active bindings are loaded as remote Runtime sources and receive signed capability tokens. Repository-external Runtime files are not registration or trust authority.

Verify:

```bash
yoi-server trust-runtime list --workspace-id '<WORKSPACE_ID>' --json
```

## 5. Start Runtime and Workspace Server

Start Runtime with the same storage flags used during Runtime identity/trust setup:

```bash
yoi-runtime \
  --bind 127.0.0.1:38800
```

For repository builds, the equivalent cargo command is:

```bash
cargo run -p worker-runtime \
  --bin yoi-runtime \
  -- --bind 127.0.0.1:38800
```

Start Workspace Server:

```bash
yoi-server serve --listen 127.0.0.1:8787
```

For repository builds:

```bash
cargo run -p yoi-workspace-server --bin yoi-server -- serve --listen 127.0.0.1:8787
```

An empty Server DB is valid. Open the Web UI, create or authenticate the Account, and register the first Workspace through the normal Workspace creation flow. Server startup does not create a Workspace from its current working directory or repository-local configuration.

## Smoke checks

Check both trust stores:

```bash
yoi-server trust-runtime list --workspace-id '<WORKSPACE_ID>' --json
yoi-runtime trust-server list --json
```

Check that Workspace Server can see Runtime workers through the authenticated path. From the CLI:

```bash
yoi workers \
  --backend http://127.0.0.1:8787 \
  --runtime-id runtime-main
```

In Web, open the Workspace UI through Workspace Server and verify that Runtime worker listing, worker creation, and Console protocol input work. The protocol WebSocket uses the same Server-signed Runtime auth path as REST control calls.

## Rotation and replacement

Identity and trust records are intentionally not overwritten by default.

Rotate Server identity:

```bash
yoi-server identity init --server-id server-main --replace
```

After Server identity rotation, every Runtime that trusts that Server must be updated with the new Server public key:

```bash
yoi-runtime trust-server add \
  --server-id server-main \
  --public-key '<NEW_SERVER_PUBLIC_KEY>' \
  --replace
```

Rotate Runtime identity:

```bash
yoi-runtime identity init --runtime-id runtime-main --replace
```

After Runtime identity rotation, Server must be updated with the new Runtime public key:

```bash
yoi-server trust-runtime add \
  --workspace-id '<WORKSPACE_ID>' \
  --runtime-id runtime-main \
  --base-url http://127.0.0.1:38800 \
  --public-key '<NEW_RUNTIME_PUBLIC_KEY>' \
  --replace
```

## Revocation

Revoke a trusted Runtime on Server:

```bash
yoi-server trust-runtime revoke \
  --workspace-id '<WORKSPACE_ID>' \
  --runtime-id runtime-main
```

Remove a trusted Server from Runtime:

```bash
yoi-runtime trust-server revoke --server-id server-main
```

## Troubleshooting

### `trusted runtimes are registered but server identity is not initialized`

The Server DB contains trusted Runtime records, but the Server signing identity file does not exist. Run:

```bash
yoi-server identity init --server-id server-main
```

If the identity was created in another environment, ensure the Server process is using the same Yoi data directory.

### Runtime accepts unauthenticated requests

Runtime only enables signed capability-token auth when both a Runtime identity and at least one trusted Server are present in its auth file. Check:

```bash
yoi-runtime identity show --json
yoi-runtime trust-server list --json
```

Also confirm the Runtime process was started with the same `--fs-root` / `--fs-runtime-dir` used for setup.

### Wrong audience or unauthorized Runtime response

Confirm the `--runtime-id` registered on Server exactly matches the Runtime identity id:

```bash
yoi-runtime identity show --json
yoi-server trust-runtime list --workspace-id '<WORKSPACE_ID>' --json
```

`RUNTIME_ID` is the token audience; mismatches are rejected by Runtime.

### Duplicate registration fails

This is expected. Use `--replace` only when intentionally rotating or updating trust material.
