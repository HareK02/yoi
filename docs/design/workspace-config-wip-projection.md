# Workspace configuration as a WIP logical filesystem

## Authority and source mapping

Workspace configuration is not the Backend host's configuration directory. The
canonical authored source is already a SQLite-backed, revisioned virtual tree:
`workspace_config_trees`, `workspace_config_entries`, and append-only
`workspace_config_tree_revisions`. `WorkspaceConfigState` combines its
`ConfigTreeSnapshot`, toolchain/schema contract and projection digest. The
`main.dcdl` entrypoint and its imports are authored files; evaluated profiles,
prompts and other projections are not replacement source files.

The builtin Workspace schema composes profile, prompts, runtime,
repository-access and Skills settings. Profile sources include model settings.
Do not assume that all imported Profile sources live under `profiles/`:
`VirtualPath` identifies authored entries, including shared imports. Skill
Markdown and text resources are entries in the same virtual tree.

The WIP provider must resolve this authority at request time, not copy it to an
OS directory, maintain a synchronized config cache/store, or register every
entry as a permanent Object. A logical attachment selects a Workspace-bound
configuration subject; it is not a Bash cwd or a host filesystem grant.

## Existing save and activation contract

The browser source-tree editor uses `ConfigCommitRequest`: a base revision and
tree digest, changes with expected entry digests, and the `main.dcdl`
entrypoint. Candidate evaluation applies changes, formats/normalizes Decodal,
and evaluates the composed schema. Commit rechecks revision **and** digest in
an SQLite Immediate transaction before replacing active entries and appending
the revision manifest. Worker operations must share that conflict domain with
UI and other Worker edits; a per-Worker mutex alone would not prevent lost
updates.

The Backend commit orchestration validates the prompt projection and
repository-access references, then commits and best-effort publishes the prompt
projection to the embedded Runtime. Saving establishes new authoritative
Workspace configuration, **not universal live application to existing Workers**.
New Worker configuration bundles are revision-bound. In-flight consumers can
retain immutable projections. Skill availability and activation are separate
contracts. A transport failure after dispatch is not proof of rollback and is
not authorization to retry a mutation automatically.

## Separate authorities and excluded objects

The following are not files of the authored Workspace source tree:

- Backend host `server.toml`, OS configuration paths and materializer state.
- Workspace identity, signing material, runtime trust/registration,
  credential stores, authentication/session/passkey/token data and private keys.
- Repository registrations and Flow records, which use separate typed APIs.
- Workspace display name and Memory language, which have independent CAS
  domains; they must not be presented as one atomic source-tree transaction.
- Worker retention/lifecycle policy and destructive administrative operations.

Repository-access **references** in authored config are not credential private
material. Granting config access does not grant secret-management operations,
filesystem access, command execution, or arbitrary Workspace access. Authored
text can itself contain sensitive information; configuration access must be an
explicit operator grant, not a consequence of possessing Read/Write Tools.
Existing secret stores must never be serialized into observation, operation
results, diagnostic bodies or history through this provider.

## Enablement and operator grants

Choose Worker WIP mode and the Feature implementation independently of authority:

```toml
[worker]
mode = "wip"

[feature.workspace_config]
enabled = true
```

The Feature defaults disabled; enabling it does not add a settings grant. An
operator authenticated as the Workspace owner grants access to a specific
registered Runtime/Worker through `workspace_config_grant_create`:

```text
POST /api/w/<workspace-id>/workspace-config-grants
{"runtime_id":"<registered-runtime>","worker_id":"<registered-worker>","access":"read_write"}
```

Use `read_only` for observation/read-only access. Grant creation is a separate
owner administration action, not an approval prompt for each edit. To revoke,
use the owner operation `workspace_config_grant_revoke` on the returned grant
ID. Worker requests carry authenticated source identity, never these IDs as a
substitute for authorization. A grant from another Workspace or another Worker
cannot be adopted by knowing a route or possessing a previous Interface.

The existing connection ledger remains authoritative. Each granted Worker has
an independently identified logical registry target over the **same** authored
configuration tree. This preserves the ordinary Workdir exclusive-owner
invariant without making separately granted Workers maintain separate config
stores. Every attachment has an opaque connection lifetime ID; alias reuse is
not identity reuse. No Runtime filesystem session is created for this target.
Detaching ends that connection's use but deletes neither configuration nor its
registry target. Configuration is not a Workdir-cleanup or command target.

## User invocation and Worker access

Select `/workspace-config` using the common Feature completion menu. Complete
its declared parenthesized argument form (close `)` to use the default access),
then send the typed chip with the request, for example:

```text
/workspace-config() このWorkspaceのモデル設定を変更して
```

The composer preserves the selection as a typed invocation chip. Ordinary text
that happens to contain this spelling is not a command. Feature preparation
calls Backend attach before dependent LLM input and displays the selected
logical subject and effective access. Failure/unknown preparation must not be
reported as a successful attachment. Optional declarative `access` values are
`effective` (default), `read_only`, and `read_write`. They restrict existing
rights; explicit `read_write` fails rather than silently downgrading. A Worker
can invoke the native attach operation at `/workspace-config` through the same
Feature/Backend handler. Neither path creates a grant or a second ledger.

The logical content root is `/workspace-config`, not `/checkouts` or a host
path. Observe exposes requested structure and available Interfaces; contents
are retrieved through Operations. Implicit directories reflect current
`VirtualPath` entries and are not independently stored directories.

## Backend transport and consistency

Worker operations use identity-bound routes under
`/api/w/<workspace-id>/workers/self/workspace-config`:

| Interaction | Route | Contract |
| --- | --- | --- |
| Resolve connection | GET root | Current grant checked even if authorized but unattached |
| Attach | POST root | Idempotent reuse of the active canonical `workspace-config` alias |
| Observe | POST `observe` | Requested paths/depth; metadata only, no source bodies |
| Read | POST `read` | Connection + path validator; canonical body/type/digest |
| Save | POST `commit` | Connection + root validator + canonical config changes/CAS |

Observe returns root revision/digest and path validators from the same snapshot.
Opaque validators bind Workspace, Worker, grant, connection lifetime, logical
path and whole-tree revision/digest. Even replacing a file with identical bytes
in a later revision invalidates older observations. Whole-tree CAS is
intentionally conservative: an unrelated UI edit can require fresh observation.
The Client holds validators; the LLM must not manage or fabricate them.

Authorization, active connection lookup and operation use share the per-Worker
session lock with detach/revoke. The canonical Immediate-transaction CAS remains
the final cross-Worker/UI commit boundary. Failed validation, stale state and
permission rejection do not overwrite configuration. Potential auxiliary
repository-secret effects and uncertain persistence are not described as a
full rollback. Sanitized failures distinguish `not_committed` and `unknown`;
a timeout/disconnect after dispatch never causes automatic mutation retry.

Structural observation is bounded to depth 0–8 and 256 returned nodes, paths to
512 bytes and each text entry to 256 KiB. Oversized observations fail rather
than presenting a truncated result as complete. Source-tree creation/deletion
is expressed through canonical changes, including atomic multi-file changes
needed to update a source together with its import/registry references. The
`main.dcdl` entrypoint cannot be deleted/renamed. These are configuration
operations, not a generic POSIX filesystem or an OS mount.

## Native operation mapping

| WIP target | Operations and model arguments | Canonical action |
| --- | --- | --- |
| Root | `attach(access?)` | Same Backend connection operation as the selected slash invocation |
| Root | `apply_changes(changes)` | One atomic create/update/delete/rename batch; source and import/registry edits can be combined |
| Directory | `create(path, content, content_type?)` | Create an entry below this directory; ancestor directories are implicit |
| File | `read()` | Return canonical text and content type, not a byte-stream or OS descriptor |
| File | `write(content)` | Update this exact entry, preserving its content type |
| File | `edit(old_string, new_string, replace_all?)` | Unique string replacement unless `replace_all` is selected |
| File | `delete()` | Canonical deletion subject to entrypoint/import validation |
| Explicit missing path | `create(content, content_type?)` | Create that exact absence-observed entry, not a substituted path |

Content type is `decodal` (default for creation) or `text`. File operation
targets come from the resolved WIP route. The root change batch uses canonical
relative source paths; it does **not** accept `expected_digest`, revision,
validator, Workspace, grant or connection overrides as model arguments. The
adapter captures metadata and fills those preconditions automatically, requiring
all paths in a batch to match one observed tree state. Unsupported/invalid
configuration mutations fail instead of performing a partial multi-file save.

Example `apply_changes` argument:

```json
{"changes":[{"kind":"create","path":"notes/readme.md","content_type":"text","content":"Configuration notes"}]}
```

These are native Interfaces only. There is no configuration-specific
compatibility Tool or fallback under `/tools`, no Bash location, and no
`wip-fs-server`/Linux real-filesystem provider substituted for these operations.

## Validation scope

Start with the changed contracts:

```sh
cargo test -p manifest --lib workspace_config
cargo test -p worker --lib workspace_config
cargo test -p worker --lib manage_workdir::wip
cargo test -p yoi-workspace-server --lib workspace_config_integration
cargo test -p yoi-workspace-server --lib config_source::tests
cargo test -p server-api --lib workspace_config
cargo test -p tui --lib backend_worker_picker
```

The Backend integration tests use production Feature completion selection,
structured `Worker::run` input, request preparation and the real Backend router
and SQLite configuration store. At the recording LLM-client boundary they
assert that attach and its durable context happened first, then use the actual
WIP Client/Host to read/edit/write/create/delete and atomically change a source
with its import. The existing Prompt reader confirms the saved projection.
Signed-route, owner-grant, readonly/revoke, detach/lifetime, multi-Worker/UI CAS,
same-content replacement, invalid configuration, storage-failure and dropped
post-save-response cases complement that sequence.

Web tests exercise the production generic composer component's declaration
completion, selection and typed submission, plus removal/literal-text behavior.
Logical-source parsers and launch/sidebar/TUI tests ensure that a settings
attachment is not treated as a repository revision or process cwd. The two
layers are complementary; the Rust fixture uses a recording LLM client and
in-process transport, not a live browser-to-provider deployment. No live
Backend/Runtime/LLM browser or physical power-loss result is claimed. Bounds
are finite input/tree and lock/request deadlines, not a promise that synchronous
configuration evaluation is forcibly preemptible at a hard wall-clock limit.

Completion validation also runs the current-source root `cargo check`, complete
changed-crate suites, the server-api TypeScript configuration and generated
contract checks, formatting and diff checks. Updating this code does not update
or restart the running dogfooding environment.
