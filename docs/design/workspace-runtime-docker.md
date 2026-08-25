# Workspace / Runtime / Docker launch boundary

Yoi's browser-facing deployment is a three-process boundary: WebUI, Workspace Backend, and Worker Runtime. Docker Compose currently exercises that boundary with locally loaded Nix-built images; it is not a separate authority model.

This document records the current design state for the split launch path, Docker image layout, and working-directory ownership rules. It is intentionally about boundaries and invariants rather than a complete operations manual.

## Process boundary

The browser launch path is:

```text
Browser WebUI
  -> Workspace Backend Server
  -> Worker Runtime
  -> Worker process
```

Responsibilities are split as follows:

- WebUI renders the workspace and submits browser-safe requests. It does not hold filesystem authority, raw runtime paths, raw socket paths, or local Worker names as operation authority.
- Workspace Backend Server is the workspace control plane. It owns workspace-scoped API identity, Runtime registry projection, Worker registry projection, Ticket/workspace records, memory backend access, and browser-safe launch options.
- Worker Runtime owns runtime-specific process spawning, Worker controller state, workdir materialization, runtime-local workdir cleanup, and Worker protocol connectivity.
- Worker process owns model turns, tool execution inside its granted scope, session history append, and Worker protocol handling.

The Backend can project Runtime and Worker state, but it should not become a hidden filesystem/runtime implementation. Runtime observations should be reconstructable from Runtime APIs and committed Backend records.

## Docker image layout

Docker images are built through Nix `dockerTools.buildImage`, not through a root Dockerfile.

Current image roles:

```text
yoi-runtime:latest
  yoi-runtime

yoi-server:latest
  yoi-server

yoi-webui:latest
  nginx serving built WebUI assets and proxying API requests
```

The Compose setup assumes those images have already been loaded into the local Docker daemon. Compose is a local wiring layer, not the builder.

Typical development flow:

```text
nix build .#docker-runtime
nix build .#docker-server
nix build .#docker-webui
docker load < result
# repeat load for each built image result
docker compose up
```

The Compose files live at:

```text
compose.yaml
docker/workspace/.yoi/workspace.toml
docker/workspace/.yoi/workspace-backend.local.toml
```

The WebUI container serves static assets and proxies `/api` to the Backend Server. The Backend Server registers the Runtime container as a remote Runtime such as `docker-runtime`. The Runtime container runs `yoi-runtime` and owns Worker spawning/materialization for that runtime.

`YOI_BROWSER_PUBLIC_URL` is the single browser-facing deployment setting used by the Server for WebAuthn, device-login URLs, cookie policy, and cookie-authenticated mutation origin checks. Compose defaults it to `http://localhost:8080`; deployments exposed through another host, port, or HTTPS endpoint must set the exact Nginx-facing origin, for example:

```text
YOI_BROWSER_PUBLIC_URL=https://yoi.example.com docker compose up
```

The value is an origin, not an API/backend URL, and must not include a path, query, or fragment.

Container user and writable data directories matter: runtime/server images must be able to write their configured data directories and named volumes. The current local-image Compose setup avoids an image-level `User` override and sets data-directory permissions accordingly.

## Worker launch path

A browser-created Worker follows this control flow:

```text
WebUI new-worker form
  runtime_id / profile / initial input / optional workdir selection

Workspace Backend create-worker API
  validate workspace scope
  resolve Runtime by canonical runtime_id
  resolve profile/manifest using Backend authority
  convert browser workdir selection into a Runtime working-directory claim
  forward a typed Worker creation request to the selected Runtime

Worker Runtime create_worker
  reject a requested primary workdir if an existing live/registered Worker already owns it
  spawn the Worker through the runtime execution backend
  return Worker detail, run state, and workdir binding

Workspace Backend post-spawn projection
  sync Worker registry
  sync workdir registry from Runtime summary
  sync Worker/workdir link
  return browser-safe Worker detail
```

The Runtime guard against assigning the same primary workdir to two active Workers is required even when the Backend has a registry projection. It is the last authority that sees runtime-local Worker requests and execution bindings.

Worker interaction uses the unified Worker protocol WebSocket path. The older event-only stream is not an operation authority. Normal interaction should go through protocol transport and runtime/backend forwarding rather than raw local sockets.

## Working-directory model

A working directory is a materialized repository for a workspace on a concrete Runtime.

The normal invariant is:

```text
workdir {
  workspace_id      # Backend workspace scope
  runtime_id        # concrete Runtime that owns the materialization
  repository_id     # repository being materialized
  working_directory_id
  selector / resolved commit
  materialization status
  cleanliness
}
```

`repository_id` is part of the normal model. A workdir without a repository is not a normal reusable workspace workdir; it should be treated as diagnostic/orphan/corrupted state rather than as a first-class launch candidate.

The removed `management_kind` state must not be reintroduced. Whether a row was first authored by Backend creation or later observed from Runtime is provenance, not current workdir authority. Reuse eligibility must be derived from current state and ownership:

- Runtime reports the workdir as active/present;
- cleanliness is clean;
- no active Runtime Worker owns it as a primary workdir;
- Backend worker/workdir links do not show an existing owner;
- `primary_worker_id` is absent in the browser-safe summary.

Runtime-observed workdirs that satisfy those conditions are valid reusable candidates for the selected Runtime. Hiding them because Backend did not originally author the registry row creates a stale provenance bug.

## Workspace and Runtime sharing assumption

The current Docker and browser launch path treats a configured Runtime as serving the Workspace that registered it. Runtime workdir summaries do not carry independent workspace authority sufficient for arbitrary multi-workspace sharing.

If one Runtime is later shared by multiple Workspace Backends, the Runtime workdir materialization record/API must carry enough workspace identity to let a Backend decide whether a workdir belongs to its workspace. That should be modeled explicitly as workspace identity on materialization records, not through a vague managed/unmanaged kind.

Until that exists, do not infer cross-workspace ownership from display names, local paths, Runtime labels, or Backend registry provenance.

## Memory backend launch pitfall

Workspace memory backend HTTP operations run inside async Worker/runtime paths. They must use async HTTP clients. Blocking HTTP clients such as `reqwest::blocking` can panic when created/dropped inside a Tokio async context and can kill Worker controller tasks.

The current memory backend HTTP path uses async `reqwest::Client`. New Backend/Runtime launch code should preserve that boundary.

## Current validation surface

The local-image Compose path has been checked by building/loading the Nix Docker images, running `docker compose config`, starting the Compose stack, loading WebUI `/`, and confirming `/api/runtimes` sees the remote Runtime.

Code-level changes around Runtime/Backend/WebUI boundaries should generally be checked with the relevant Rust crates, WebUI check, and whitespace validation, for example:

```text
nix develop -c cargo check -p yoi-workspace-server -p worker-runtime -p client
nix develop -c cargo test -p yoi-workspace-server --lib
cd web/workspace && deno task check
git diff --check
```
