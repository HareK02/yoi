# SubWorker / Workdir execution ergonomics during T-730

Observed during Workdir removal implementation in a dedicated clean checkout:

- Children using `profile: builtin:default` were given explicit writable scope
  and command permission, but their available tools/context did not support the
  requested implementation. They performed read-only investigation and reported
  the missing capabilities. Recreating coding children with `profile: inherit`
  exposed the needed implementation/Ticket tools. No rejected capability was
  bypassed.
- Concurrent Workdir reads sometimes returned `filesystem provider busy`.
  This is transient provider contention, not an authorization rejection. A
  child initially treated it as a stop condition; a subsequent parent turn
  clarified the difference and sequential retries proceeded normally.
- After a user interruption, repository changes and parent tasks remained, but
  `WorkerList` no longer listed the previously active children. Their work was
  not treated as approved or complete: the parent inspected concrete diffs,
  reran Web/browser validation and delegated a fresh bounded Runtime follow-up.

Potential improvements:

1. Show effective child tool capabilities together with filesystem/command
   grants before spawn, so a writable scope is not mistaken for an available
   implementation tool surface.
2. Distinguish retryable provider contention from capability rejection in typed
   error metadata and recommended recovery text.
3. Make interruption disposition and retained child-session observation grants
   explicit. Preserved files are useful evidence, but they do not replace a
   committed final child report or verification of tests.

These observations concern development-tool ergonomics, not product removal
requirements or permission to operate on live Workdirs/mounts. The T-730 work
uses temporary fixtures and ordinary Git publication; no live cleanup, unmount,
registry edits or runtime deployment were performed.
