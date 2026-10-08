# T-722: SubWorker cwd and logical path friction

During delegation from the assigned repository Workdir, a child started with
`cwd: crates/server-api` and readable policy paths under repository-root
`docs/development`. Its Read calls using repository-relative logical paths could
not read the required testing strategy; absolute paths were rejected as invalid
logical paths too. The child correctly stopped without edits. The parent
inspected the committed child session, stopped that idle child, and recreated it
with repository-root cwd and explicit read-only repository scope plus write scope
limited to server-api. Policy reads and implementation then succeeded.

The current observable behavior appears to root child filesystem tools at the
selected cwd, rather than making cwd only the command default as the spawn-tool
description states. This makes repository-root logical paths surprising. Consider
keeping the attachment root invariant and applying cwd only to command execution,
or documenting/returning a distinct filesystem base explicitly. Do not encourage
absolute paths, parent traversal, or broader write grants as a workaround. This
report records a tool usability issue, not additional Ticket requirements.
