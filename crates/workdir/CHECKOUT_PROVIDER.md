# Request-time checkout provider boundary

`workdir::checkout` (also crate-root reexports) exposes `CheckoutObservation`,
`CheckoutRequest`, `CheckoutOperation`, `CheckoutOutput`, and `CheckoutResult`.
`WorkdirSession::checkout_observe(path)` defaults to unsupported; execution also
fails closed by default. Observation is metadata-only, with no recursive tree
walk. Directory discovery/search remains the ordinary bounded List/Glob/Grep
provider engine. GrepResult.paths carries exact typed provider paths appearing
in the returned report, respecting its offset/limit/truncation; filenames are
never parsed from rendered text. List/Glob/Grep skip unrepresentable non-UTF8
paths, preserving valid Unicode exactly without normalization. Native callers supply their own attachment generation/namespace
fence; this provider validator is not a substitute for that fence.

* Read: line `offset`, `limit`, `max_bytes`; same `ReadResult` (bytes, start line,
  total lines, full-source content hash, truncation) as normal Tools. Formatting
  line numbers and recording the returned hash in caller read-history are caller
  responsibilities. The validator is **not** permission to bypass read-before-write.
* Write: replace existing file with `content` and required `expected_hash`.
* Edit: existing UTF-8 file, unique `old_string` unless `replace_all`, required
  `expected_hash`; same match/replacement/result-size checks as Tools.
* Create: target must be an observed directory; `path` is **checkout-root
  relative**, strictly below target, not target-relative. No existing file can
  be overwritten. Intermediate directories and destination must be writable.
* Every result observation describes the bound request target. Create returns
  the updated **parent directory** observation from the retained pinned target,
  not the created destination. Read returns the read state's observation.

## Shared implementation

`fs_operation::CheckedTarget` is an `FsAccessPolicy` adapter around pinned target,
save-parent, and provider-root descriptors. It calls the ordinary
`run_read_bounded`, `run_edit`, and `run_write` functions, including full-source
hashing, UTF-8/string matching, bounds, and atomic temp-save primitive; there is
no second content-operation engine. Shared text reads now check descriptor
metadata before/after reading. Validator bytes encode device/inode/size/mode/
mtime/ctime with nanoseconds, excluding atime and host paths; same-content inode
replacement is distinguishable. Successful native Read rechecks the named target
and pinned descriptor before returning hash and observation.

Local sessions serialize checked observe/execute and ordinary trait Read/Write/
Edit using one nonqueueing session lock. Expected validator comparison happens
inside that boundary. The adapter retains handles across reading and save,
rechecks named target + save-parent identity at save boundary, and writes a new
temporary inode then atomically renames it, isolating existing hardlink aliases.
External sessions retain the approved descriptor root after pathname rename or
replacement. Checkout resolution rejects symlinks, special files, mount crossings,
absolute/traversal paths and unavailable descriptor primitives, rather than
weakening confinement. This currently requires Linux `openat2` and `/proc/self/fd`.
Ordinary Tools preserve their existing explicit symlink-policy behavior.

Scoped sessions use their existing scope mutex, current validity/source
capability checks, provider authorization and write leases; Create also checks
intermediate paths. Observation capabilities are attenuated per path and live
write leases, never broadened to the whole attachment. ReadOnly removes mutation
capabilities and rejects mutations. Routed operations use existing active-op
accounting and detach fencing. Remote HTTP forwards exact operation/result types
and validates returned operation/path pairing. External DTOs validate checkout
path/validator/content/result bounds. No provider endpoint or host root is added
to the observation.

## Effects and limits

Absent paths during checkout observation return `NotFound` (including a missing
ancestor or a non-directory ancestor); native discovery can return no route.
An execution against a previously observed target that disappeared instead
returns `Conflict`, consistently at pinning, shared-engine, and final-check
boundaries. Conflict means stale validator/hash/name or existing Create destination before
save effects. `WorkdirError::OutcomeUnknown` has a separate sanitized wire code
`outcome_unknown` (HTTP 500), never an automatic retry. Partial parent creation,
possible commit failure, or failed post-save observation is outcome unknown.
Remote lost/malformed/mismatched mutation responses are also outcome unknown.
Create failures conservatively report unknown once parent creation may begin,
even if a particular failure happened without a persisted effect.

Bounds: path <=4096 UTF-8 bytes/128 components, validator <=256 bytes; Read source
64 MiB, retained response <=1 MiB (or stricter provider read limits), Write/Edit
preimage and result <=8 MiB (or stricter provider limit), Edit replacements
<=10,000 (or stricter provider limit). One checked operation per Local session,
with bounded retained handles and no waiting queue. Cancellation/deadline checks
use the existing provider guard, or a 30-second cooperative guard for Local
checkout operations. Blocking kernel/filesystem calls are not preempted; this
is **not** a hard wall-time guarantee for hung storage. Transport request-body
bounds before serde allocation remain the hosting transport's responsibility.

### External-editor limitations (not a magic advisory-lock guarantee)

No advisory `flock` is claimed to exclude arbitrary editors. Provider-session
serialization does not lock other sessions/processes. Metadata checks detect
concurrent reads/edits up to each check, but are not a transactional snapshot
against privileged actors forging metadata. Linux offers no atomic
compare-identity-and-replace-by-name primitive here: an uncooperative external
rename/change between the final validator check and rename can still race the
save. The pinned handles prevent following replacements/symlinks into arbitrary
objects, and deterministic substitution before the save boundary is rejected,
but the final syscall gap is an explicit limitation. Callers must not interpret
this as linearizability against all external filesystem mutations. Likewise,
post-save observation is the then-current named state, not an exclusion of an
external edit after save. Crash durability of parent directories is not claimed.

Host generation reuse, native permission identity, read-history updates, model
Operation publication, transport hosting (Runtime/Backend/External), and durable
Ticket/MR review remain the parent integration's responsibilities.

## Bounded live List pagination

`ListRequest { path, limit, after: Option<ListCursor> }` accepts an exclusive
ordering key `ListCursor { kind: EntryKind, path: FsPath }`. Directories sort
first, then paths sort lexically within the directory/non-directory groups;
File, Symlink and Other share the non-directory group. `ListResult.next_after`
is the last returned key when another page remains. Reuse that key unchanged
with the same directory and output root. Cursor paths must name a direct child
in the result coordinate frame; they are not statted and need not still exist.
The optional fields default to absent and are omitted from JSON when absent.

The shared provider engine retains at most `limit + 1` candidates, additionally
bounded by the 1 MiB aggregate path-response ceiling (including continuation),
while scanning the directory once. It never stops merely because the page is
full: enumeration order cannot determine the retained prefix. `total_entries`
and `total_bytes` describe all visible entries in the current scan, including
keys at/before `after`, not only the page or remaining suffix. A byte-bounded
page may contain fewer than `limit` entries and still provide continuation.
A zero-limit request returns totals and truncation but no continuation.
List uses Read capability and read/enumeration scope, and **does not ignore-filter**
`.gitignore`/`.ignore` entries or hide dotfiles; Glob/Grep keep their own policy.
Cancellation and the 100,000-entry traversal ceiling apply to the complete scan,
including excluded entries. External DTOs retain their existing item/path bounds
and also validate cursor paths and continuation response bytes.

Each page observes a live directory, not a snapshot, and no cross-page
consistency is promised. Insertions after the cursor may appear; earlier keys
are not revisited. Removed entries disappear, including the cursor entry.
Renames or kind changes can move an entry across the cursor and cause omission
or repetition. Even one scan is not transactional against external editors.

## Scoped checkout search boundary

`CheckoutSearchRequest::new(CheckoutSearchOperation::{List, Glob, Grep}(request))`
uses the current session's logical root as its result frame. The typed result is
`CheckoutSearchResult::{List, Glob, Grep}`. The request's internal context fields
are `scope_layers: Vec<Vec<WorkdirToolScopeRule>>` and `output_root: WorkdirPath`.
Rules union within each layer; layers intersect with each other, the current
provider authority, and the output-root boundary. Wrappers contribute their own
scope; caller-supplied layers can only narrow, never replace it. Each wrapper
translates incoming provider coordinates once and forwards the composed output root.
List cursors remain output-root-relative, just like result paths.
Provider-rendered Grep text and every typed result path are already relative to
that final root; wrappers must not prefix cwd onto returned paths again.

Local/External use the same shared `run_list`, `run_glob`, `run_grep` engines.
`SearchAccess` gates metadata/content opens and enumeration. Shared List checks
`can_enumerate_directory`; Glob/Grep use the shared bounded descriptor walk and
owned `traversal_filter` policy to prune before descending/opening directories.
The walk anchors at the actual search directory, not at a broader provider
root. It counts rejected entries too, checks cancellation while scanning them,
and limits descent to 128 levels (bounded retained directory handles). Existing
provider-owned ignore loading, glob/type selection, grep context/offset/limit,
rendering, and source/result limits remain in their shared engines. Unreadable
ignore files are not opened to discover out-of-scope contents.

Descriptor search loads `.ignore`, `.gitignore` and an authorized directory's
`.git/info/exclude` through the same `FsAccessPolicy` opens as other sources.
It preserves anchored rules, negation, nearest-directory precedence within a
source class, `.ignore` precedence over Git rules, and nested repository
boundaries. Ignored directories are pruned before enumeration or loading their
own rules; explicitly selected ignored search roots are still traversable.
Only the current ancestor matchers are retained, never a second tree/store.
Glob requires an authorized `.git` directory/file marker for Git rules. Grep
retains the existing descriptor-provider contract of applying `.gitignore`
without a marker and rejecting unavailable/symlinked ignore opens; Glob skips
unavailable ignore sources like its ordinary walker. Scope-denied sources are
skipped by both before open. Bounds/cancellation failures are never retried.

Configuration discovery cannot expand provider authority. Ambient ancestors
outside the search result/provider root, global Git configuration/ignore files,
and linked-worktree `.git`/`commondir` administration pointers are not followed.
A `.git` file activates repository matching without treating it as a directory
or opening the host path it contains. In-root readable ignore rules remain
active, including inherited rules above a nested base. Descriptor ignore text
must be UTF-8, bounded to 1 MiB/file, 16 KiB/line and 8 MiB aggregate per search;
limit violations fail rather than returning silently incomplete filtering.
Grep's separate candidate-content budget remains 64 MiB. Ordinary unscoped
path-backed Tools keep their original discovery policy.

Scoped ordinary trait List/Glob/Grep use this boundary as well, returning
session-logical coordinates. Unscoped normal Tools continue using their original
trait/provider behavior. A provider without checkout search support returns
UnsupportedOperation; there is no broad traversal fallback. Native/scoped
searches intentionally fail closed on symlink paths, including logical-policy
aliases which normal unscoped Tools may follow. A visible symlink may therefore
cause a native List failure rather than expanding authority. Descriptor support
is currently Linux-only. Search remains request-time traversal, not a snapshot
or transactional exclusion of external editors.

HTTP operation/result variant: `CheckoutSearch`. External DTO validation rejects
absolute/out-of-root paths, empty scope layers, oversized paths/patterns/context/
results and invalid result paths. Generic request ceilings are 16 layers and 256
aggregate rules; External transport further limits aggregate rules to 64. Search
result limits follow the existing shared List/Glob/Grep wire bounds. Runtime and
Backend forwarding outside this crate remain parent integration work.
