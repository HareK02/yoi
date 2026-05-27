# File references and symlinks

FileRef resolution and file tools follow symlinks only after the resolved target passes the Pod scope check. A symlink placed inside the workspace does not grant access to the target by itself.

Recommended external-reference workflow:

- Prefer adding the real external project path, such as a local external checkout, to the Pod read scope when the Pod is started or spawned.
- If a workspace symlink is used, the symlink target still must be inside readable scope. For writes, the resolved target must be inside writable scope.
- If a relative symlink is broken, recreate it with the correct relative target from the symlink's parent directory, or use an absolute symlink.
- Directory traversal tools such as Glob and Grep do not follow symlink directories. Use the resolved target directory directly when it is in read scope.

This preserves symlink escape safety: access decisions are made on the canonicalized target whenever the target exists, and broken or out-of-scope symlinks are rejected with diagnostics that include the original path and target where possible.
