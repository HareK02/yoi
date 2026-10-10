# Standalone local Subjects

Standalone runs one client-owned Worker in the current process, without a Backend or Workspace. Local Subjects are explicitly created and selected; enabling a subjektiv Profile alone neither creates nor attaches a Subject.

## Create and list

```sh
yoi --local subject create assistant --behavior-md 'Keep durable decisions and preferences.'
yoi --local subject list
```

`create <ROLE>` prints the created Subject record as JSON. `--behavior-md <TEXT>` is optional inline text, not a file path. `list` prints a JSON array of records in the selected standalone state scope. Both commands require explicit `--local` and reject Backend/Workspace selectors.

The state directory is obtained from the same local connection target as normal standalone launch. Its base follows the existing `YOI_DATA_DIR`, `YOI_HOME`, `XDG_DATA_HOME`, or `HOME` resolution, with `client/standalone/workers` appended. Subjects live under `<state_dir>/subjektiv/`; cwd, repository files, Worker names, and Profiles are not Subject identity or sharing boundaries. Use the returned common-domain Subject ID (`subject-<UUID>`), not the role, for selection.

The user-managed state root must be operator-owned and private (mode **0700**). A missing root is created with mode 0700; an existing root is validated, not automatically chmodded. An older nonprivate root is rejected. The operator can explicitly run `chmod 700 <state_dir>` on their own state root before retrying; do not substitute a repository directory or assume the CLI repairs permissions. Symlink and nonprivate Subject storage nodes are also rejected.

## Select for a new Worker

```sh
yoi --local --subject <ID> --profile builtin:standalone-subjektiv
```

`--subject=<ID>` is also accepted. The selector must be nonempty and may only be supplied once. It applies only to fresh local spawn: Backend selection, Runtime attach, socket/session attach, and resume combinations are rejected. A selected Subject requires a compatible subjektiv policy; launch validation rejects incompatible policy before Worker startup side effects.

The interactive local spawn form has an optional `subject` field, initially `(none)`. Press **F2** to switch between editing the Worker name and Subject ID; **Tab/Up/Down** continue to cycle Profiles. Leave the Subject field empty for the existing Subject-free launch path. The form does not create Subjects or open Subject storage just to display the field. The optional built-in `builtin:standalone-subjektiv` Profile enables local Subject behavior; its consolidation Profile is `builtin:standalone-subjektiv-consolidation` and does not require Backend resources.

## Resume

```sh
yoi --local resume
```

Resume restores the original persisted Subject/storage-scope binding and reacquires its execution lease. It does not accept a new `--subject` selector or silently change Subjects. The Host holds an exclusive per-Subject kernel lease for body execution and delegated Jobs until confirmed shutdown; one Subject cannot be executed concurrently by two processes. Process death releases the kernel lease, while uncertain in-process cleanup retains ownership. Separate Subjects may run concurrently in the same state scope.

## Subject Jobs and recovery

A selected Subject uses `<state_dir>/subjektiv/jobs/<SubjectId>/jobs.sqlite3`, shared across Worker IDs for that Subject and protected by its exclusive execution lease. Subject-free Hosts retain the existing Worker-local Job location. There is no separate Subject runner or background daemon.

On explicit selected-Subject Host startup, reserved Jobs resume automatically. An interrupted Job whose execution outcome is unknown is **never automatically replayed**. A successful result that has not yet been acknowledged is delivered from durable storage without retrying the model. Recovery is driven by an explicit Host startup, not an automatic process restart or independent scheduler.

## Memory and historical Sessions

The normal body Profile has recall, exact Memory change reads, explicit candidate tools, and bounded historical Session exploration. It cannot directly apply Memory. Explicit Remember/Revise without `entry_refs` returns a pending-commit receipt; the Host stages its exact tool-call evidence only after the logical Run is append-only committed. Pre-request retry may defer an open Run, but commit/rewrite failures are not silently treated as success. A candidate remains unconfirmed until the restricted consolidation Job applies or rejects it using the shared CAS/receipt rules.

Normal extraction uses the existing committed-capture lifecycle, restricted extraction tools, and append-only `subjektiv.extract.v1` pointer. The selected Profile owns extraction thresholds/model policy. An explicit extraction model override chooses that provider, not an injected dialog transport. Local Memory language defaults to English and is stored in operator-managed `subjektiv/language.json`, never loaded from repository policy files.

The Host stamps human input on its owned in-process client channel as persisted `local_human_input`, projected as the existing `HumanInput` provenance. It neither invents a Backend account nor trusts serialized source claims. Ordinary SubWorker/Job input retains its own nonhuman origin. Local evidence has no Workspace/account/Runtime identity; local storage-scope identity and Worker binding authorize discovery separately. Historical Sessions are not copied into SQLite. Reads/search exclude system prompts, hidden reasoning, incomplete logical Runs, partial JSONL tails, and foreign bindings; cursor continuations revalidate scope/Subject/Worker attribution and committed generation.

## Validation boundary

Host injection tests pass a clone of the injected model transport into delegated Jobs, so scripted body and Job calls use the same test-owned transport. These are local contract tests, not live-provider validation or evidence of model/consolidation quality. Generic Job finalizer and Host integration proofs are validated separately from the CLI/parser and spawn-form tests.
