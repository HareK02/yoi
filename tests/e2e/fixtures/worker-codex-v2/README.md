# Current-source restore fixture candidate

Captured from clean source `e40316bac9507fc772a8f960fe58da7bb71104bd` by
`tests/e2e/examples/acquire_restore_fixture.rs`, using isolated featureless Server
and Runtime binaries and two real Codex OAuth conversation turns. This is an
artificial passphrase conversation, not copied production history.

The acquisition checked the live public snapshot against the offline projection,
verified the original Session bytes and source-file hashes, and validated the
published manifest. Credentials, source Worker metadata and execution grants are
not included. The source processes and temporary roots were cleaned up.

This candidate is **not yet the default fixture**. Select it explicitly with
`YOI_E2E_RESTORE_FIXTURE`. The first featureless restore test run passed the seed
projection unit test but failed all four process cases at the restore API boundary
(direct requests returned HTTP 403; response-loss relays rejected non-200 replies).
The old offline Backend seed creates catalog ownership but no current execution
authority, which the current restore path requires. Fix and validate the isolated
seed through the normal authority model; do not bypass restore authorization.
Checkpoint process tests have not yet run with this candidate.
