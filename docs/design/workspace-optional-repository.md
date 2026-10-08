# Workspace creation with optional Repository intent

A Workspace is usable without a Repository. Its initial Git Repository is an
optional asset, not an identity or a prerequisite for connection. Creation does
not initialize a Git repository or infer a replacement asset.

## Creation and replay contract

`POST /api/workspaces` accepts `operation_key`, `display_name`, and an optional
`repository` intent. Omitted and explicit `null` both mean no initial Repository.
An object is validated normally: a partial object, invalid URI, or invalid key is
not treated as omission. Ownership comes from the authenticated request actor,
not a caller-supplied owner field.

The bootstrap transaction persists the Workspace, optional Repository, initial
config, memory settings, Ticket/Objective/Worker resource-key counters, active
signing identity, and create receipt together. Signing-material provisioning
retains its existing durable reservation/recovery protocol; a failed bootstrap
leaves no partially created Workspace or Repository.

The fingerprint includes normalized creation intent and owner identity. No
Repository differs from any Repository intent; repository-present fingerprints
keep the existing encoding so saved create receipts remain replayable. The same
operation key and input converge, including concurrent calls; changed input is a
conflict. No schema migration is needed for the existing fingerprint/Workspace
receipts.

A replay is not a historical snapshot. It returns the current Workspace and
config revision. If creation supplied a Repository, it returns the current
record **at the originally requested key**. Removal or renaming away from that
key produces `repository: null`; registration at that key later returns that
current record. Replay never re-registers or resets an asset. If creation omitted
a Repository, replay always returns `null`, even after other assets are added.

## Entry points and zero-Repository use

- Web creation requires only a display name. Optional Repository fields are
  disclosed explicitly; Settings retains the later-registration entry point.
- `yoi [--backend URL] workspace create --display-name NAME` creates without Git
  discovery. Select the returned identity with `yoi --workspace-id ID`.
- `yoi init` remains Git-oriented and still requires an explicit Repository key.
- The TUI creation picker allows Enter at the optional URI prompt to skip Git
  registration. Workspace selection and ordinary embedded Worker startup do not
  require a Repository or Workdir. Operations that use Git/Workdirs retain their
  own prerequisites.
- Empty Repository lists are valid, without a missing-configuration warning.
  Reopening and deletion use the usual Workspace authority and lifecycle guards;
  in particular, the last-accessible-Workspace deletion guard is unchanged.

Optional creation preserves the existing owner/member responsibility split:
settings are owner-controlled, while ordinary viewing and use are member-facing.
Non-ownership alone is not a reason to deny ordinary viewing. Existing
member-facing non-owner reads, owner-gated settings mutations, owner-scoped
catalog discovery, authentication and operation-permission checks are unchanged.
This creation change does not introduce a membership authority or an owner-only
viewing filter.
