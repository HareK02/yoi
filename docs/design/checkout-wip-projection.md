# Attached checkout WIP projection

T-699 publishes the contents of **current Worker attachments**, independently of
Repository/Workdir inventory and management. This is a Yoi implementation of
request-time projection, **not a dependency on `wip-fs-server`**. The upstream
read-only source/specification informed the design; its byte `read` and
truncate/write `replace` contracts are not substituted for Yoi's text/edit/save
contracts. No file-tree store, synchronization, or per-file resident registration
is created.

```text
/
├── repositories                  inventory/reference
├── workdirs                      identity/lifecycle
├── workdir-attachments           Backend connection lifetimes
└── checkouts                     current accessible content entrances; list
    └── <encoded alias>           directory; glob, grep, create_file
        ├── README.md             file; read, edit, write
        └── src                   directory; glob, grep, create_file
```

Operations are Interface declarations, not children or `/features/...` routes.
File tools are native when the normal Worker WIP surface is installed. Bash stays
in compatibility mode. Ordinary Tools mode retains Read/Edit/Write/Glob/Grep.
Feature activation never grants READ, WRITE, EDIT, search, COMMAND, or delegation.

## Placement and ownership

`worker::checkout` owns Worldspace binding, contextual Interface declarations,
permission input reconstruction and safe output. It never opens a host pathname.
It resolves the live `WorkdirSessionRouter` on each request. The existing
`WorkdirSession` provider executes `checkout_observe` and `checkout_execute`;
Runtime HTTP and External transports carry typed operation/result envelopes at
their existing authorization, connection-generation and cancellation boundaries.
The Workspace-attached session forwards through the existing Backend broker.
Endpoint paths, credentials and materialized roots do not enter WIP descriptors,
Objects, arguments or results.

The common Host registry has an **explicit asynchronous subtree provider**
contract, separate from the existing one-segment dynamic item families. It
validates every resolved projection's path/owner/descriptor and every enumerated
child's immediate-parent relationship. Static and dynamic/subtree overlaps are
rejected. Observe resolves only the requested depth; successful observations are
complete through that depth, not partial trees masked as success. Directory
listing and native search use the shared List/Glob/Grep engines through typed
`CheckoutSearchRequest` forwarding. Scoped sessions append intersecting rule
layers, translate their cwd, and require provider-side visibility checks before
entry descent/content reads. Results retain session-logical coordinates, so a
SubWorker cwd does not duplicate a path prefix in its Object links. Unsupported
providers fail closed rather than falling back to a broader source search.
Direct resolution also uses current provider authority, not possession of a
cached Interface.

## Names and connection isolation

Aliases use T-697's canonical encoding: nonempty ASCII letters/digits/`-`/`_` are
unchanged; every other UTF-8 alias is `~` plus lowercase UTF-8 hex, with `~` itself
encoded. This is injective, not display-name slugification, Unicode normalization,
URL decoding or case folding. Only canonical spellings are accepted.

`checkouts.list` returns alias, slug, Workdir ID, Worker-local attachment generation
and `path`. Attachment inventory retains its Backend connection lifetime ID and
can return `checkout_path`/`checkout_slug` for the matching live router attachment.
Attach results point to the attachment catalog and checkout entrance. Workdir
existence alone does not expose its content. Following a reference still requires
current scope/capability and an accessible filesystem projection.

Objects have no shared inode `ref`: hard-linked names and different attachments
must not be mistaken for one path-independent representation. Interface references
are opaque, path-qualified and include a fresh projection incarnation and live
attachment generation. Validators combine that namespace with the provider's
opaque identity/state validator. Restoring a Worker creates a fresh incarnation;
detach/alias reuse changes the router generation. An old Interface, validator or
read-history entry cannot retarget a new connection. No separate connection ledger
is added. The existing router's active-operation exclusion and begin/finish/cancel
detach paths remain authoritative; detach does not remove the Workdir/files.
Collection enumeration rejects connection-set changes between sampling and
publication instead of pairing old items with a new validator. Search-link
publication checks the captured attachment generation, not merely a reused
alias; response-side provider awaits are covered by the operation deadline.

## AI Operation mapping

All operations bind their attachment and target from the WIP route. There is no
`target_workdir`, Workdir ID, host path or replacement `file_path` argument.
Arguments not declared by the descriptor are rejected before dispatch. Paths
below are relative to the **bound directory**, not the Worldspace or endpoint.

| Object | Operation | Arguments | Result |
| --- | --- | --- | --- |
| `/checkouts` | `list` | none | bounded `items` containing alias/slug/Workdir/generation/path |
| directory | `glob` | `pattern`, optional `path` | normal bounded Glob text/summary plus same-checkout Object links |
| directory | `grep` | `pattern`; optional `path`, `glob`, `type`, `case_insensitive`, `-A`, `-B`, `-C`, `multiline`, `output_mode`, `head_limit`, `offset` | normal Grep grouped/context/count text and typed provider search targets mapped to Object links |
| file | `read` | optional line `offset`, `limit` | line-numbered text and normal Read summary |
| file | `edit` | `old_string`, `new_string`, optional `replace_all` | replacement count/summary and preview |
| file | `write` | `content` | existing-file save summary; never creation |
| directory | `create_file` | `path`, `content` | create-new summary and created-file link; permitted missing parents are created |

Native operation results use a JSON record with `summary`, nullable `content` and
`items` (`path` is an absolute WIP Object navigation coordinate, not an OS path).
Search links are constructed from **typed provider paths**, not by parsing Grep's
rendered filenames/line delimiters. Provider scope remains authoritative for
search output as well as links. Glob/Grep share normal provider ignore/pattern,
context/output mode, offset and bounded result behavior. Search does not claim an
atomic snapshot of every descendant: it fences the bound directory before/after
search and each provider read/search retains its own consistency guarantees.

Read offsets are zero-based **lines**, limit defaults to 2000, displayed numbers
are one-based. The shared Read retains at most 256 KiB of source text and bounds
its numbered presentation to 256 KiB, with an explicit truncation marker when
numbering/Unicode expansion exceeds that budget. No byte-range
operation is published by this native surface. Partial Read records the provider's
full bounded-source hash and total line count, not the hash of only its displayed
slice. Native and ordinary Tool calls use the same original alias/generation
tracker, mutation coordinator, rendering and parameter validation.

Edit requires a nonempty, different `old_string`/`new_string`; `old_string` must
occur exactly once unless `replace_all` is true. Existing Write/Edit require a
prior successful Read through the same attachment. The read-history hash check
and the provider identity validator are **both** required: a same-content inode
replacement cannot be blessed merely by matching a hash. Failed/conflicting Read
must not record history; successful mutations update hash/change statistics only
after provider success. Create is strictly beneath the bound parent, creates only
an absent file, and must not overwrite an existing destination or symlink.

Manifest permission identities remain Read/Edit/Write/Glob/Grep; create_file uses
Write. The Host reconstructs the same route-bound input for policy evaluation;
provider read/write scope, capabilities and delegated leases are checked again.
`ask` fails closed. Native replacement claims remove `/tools/Read`, `/tools/Edit`,
`/tools/Write`, `/tools/Glob`, `/tools/Grep`, not unrelated Tools.

## Consistency, safety and resource policy

The provider resolves/pins the entry/parent, compares its opaque validator and
executes shared `fs-operation` processing within the same checked boundary. The
validator represents identity and protected metadata, independently of the Read
content hash. Read checks state across its stream and returns a validator for
that same state. The WIP response uses the **execution result's validator**, not a
later independent path stat that could bless a replacement after execution.

The native checked filesystem projection deliberately rejects symlink traversal
and unsupported/special entries. It does not silently broaden existing resolved
or logical scope. Ordinary Tools retain their existing symlink-policy behavior.
External sessions retain the root descriptor pinned **before approval/grant**;
renaming/replacing its ambient pathname must not change the approved root.
Absolute paths, traversal, alternate alias arguments and special-file mutation
are not native capabilities. UTF-8 names are byte-for-byte; non-UTF-8 names must
not be lossy-normalized into another Object name. Root/directory permissions are
not inferred from catalog visibility or from a descendant pathname supplied by
the model.

Saves retain Yoi's temporary-file publication rather than in-place truncation.
Thus saving one hard-link pathname replaces that name rather than changing all
aliases of the old inode. Checked preconditions detect known changes before
commit. Advisory locks cannot exclude arbitrary noncooperating external editors;
read-time/commit-time identity checks do not create a filesystem-wide transaction.
The supported race guarantees and remaining external namespace race window are
specified by the provider implementation and tests, not by a claim that a pinned
fd makes any rename-based update universally atomic.

A guaranteed pre-effect rejection is distinct from **OutcomeUnknown** after
mutation/parent creation may have started, commit succeeds but state cannot be
returned, response validation/encoding fails, or a transport/deadline loses the
result. Never automatically retry uncertain mutations. Inspect current state and
rediscover/reread. Do not report partially created parents as unchanged.

Host observations are bounded to depth 32 and 1024 nodes with a 30-second total
cooperative deadline. The checkout adapter permits at most 16 concurrent requests
and bounds admission waiting and the complete operation (including result-link
publication) to 30 seconds each; provider path/depth, source/response,
mutation/search and transport bounds apply too. No initial context expands an
entire tree. Resource-limit errors do not turn incomplete observation into a
successful truncated tree; bounded search results may explicitly report
truncation under the existing search contract.

## Typical sequence

1. Discover `/checkouts` with depth 1; inspect its Interface and call `list`.
2. Discover an entrance or known deep directory, inspect it, call `glob`/`grep`.
3. Follow a returned file `path`, discover/inspect, call line `read`.
4. Call `edit` or `write`; Client-managed validators protect the exact observed
   object while the shared tracker enforces the prior Read.
5. Rediscover after external changes; a stale rejection is not permission to skip
   Read. ReRead after successful save to verify text.
6. Inspect a directory and call `create_file` with a descendant relative path,
   then discover/Read the returned new file.
7. Use `/workdir-attachments/<connection lifetime>` for detach. Old checkout
   observations and history cannot operate a newly attached alias.

Workspace configuration (T-695) remains a separate logical provider; it is not
replaced by raw Linux filesystem writes. Bash native conversion, broad POSIX
operations, lifecycle duplication, upstream changes and deployment/restarts are
outside this feature.

## Reproducible validation matrix

Run from the main repository, on Linux with `openat2` and `/proc/self/fd`:

```sh
cargo check --workspace --all-targets
cargo test -p fs-operation -p workdir -p tools -p worker
cargo test -p worker-runtime -p yoi-workspace-server
cargo fmt --all -- --check
git diff --check
```

| Boundary | Focused regression locations |
| --- | --- |
| Deep mounts, multiple aliases, route-only arguments, exact permission identities, no duplicate compatibility file Tools | `worker/src/checkout_wip_tests.rs` and existing `wip.rs` registry/controller tests |
| Native search → numbered Read → unique/all Edit → Write → Create; returned validator cached chains | `checkout_wip_tests.rs`, `tools/src/checkout_tests.rs` |
| SubWorker write leases, sparse/nonrecursive/nested scopes, denied basename/content pruning, result-coordinate rebasing, revocation | `workdir/src/checkout_search_tests.rs`, `checkout.rs`, native scoped-child test |
| Approved External root replacement; same-content inode/name/parent substitution; hard-link save isolation; symlinks/special files; Unicode/non-UTF8 paths | `workdir/src/checkout.rs`, `fs-operation/src/checked.rs` |
| Mid-read updates; independent hash and validator fences; parent partial creation and post-save state failure | `checked.rs`, `checkout.rs`, Tools tests |
| Detach/alias reuse/restore references and read history; collection/search response generation fences | `checkout_wip_tests.rs`, `checkout_race_tests.rs`, existing router/scope/catalog lifecycle tests |
| Traversal depth/entry/cancellation bounds; stalled observation/link publication; numbered output expansion; post-dispatch encoding loss | `fs-operation/src/traversal_tests.rs`, `checkout_race_tests.rs`, `tools/src/read.rs`, `wip.rs` |
| Real prefixed HTTP provider → Remote session → router → native WIP; auth, read-only, lost/malformed post-commit response without retry | `worker/src/checkout_http_tests.rs` |
| Workspace proxy/External typed pairing and unknown-outcome propagation; Backend/Runtime Workspace, grant and generation isolation | checkout additions in `manage_workdir.rs` and Backend `server.rs`, plus full existing host/provider suites |
| Ordinary Tools text/hash/edit/write/search and provider symlink policy | full Tools, fs-operation and Workdir suites |

The HTTP fixture tests actual network/codec boundaries but is not a production
Backend grant-network end-to-end deployment. Production routing/authorization is
covered separately by the existing host suites. Fault tests are deterministic
boundary simulations, not power-loss tests. Metadata/pinned-handle tests do not
close the external-editor final-check/rename window documented above. No live
Worker or deployed environment needs updating/restarting to execute this matrix.
