# subjektiv

Shared Host domain implementation for per-Subject Memory, change history, candidate dispositions, derived surfaces, and Session activity Journals.
`feature-storage` manages physical SQLite storage; the calling Host handles authentication and execution authority.
Journal storage does not generate prose, launch Jobs, or fetch Session history.
Hosts retain body ownership across inactivity, reserve actual execution quiescence, and reconcile final committed history before releasing a Journal writer lifetime. [`journal::binding`] fences new mutations inside storage transactions; historical attribution and exact receipt recovery cannot renew a released lifetime. [`journal::startup`] selects bounded reference context independently of source retention and long-term Memory.

## Main entry points

- [`SubjektivStore`]: scope-bound records and transactions
- [`MemoryRecord`], [`SurfaceSnapshot`]: change history and derived context
- [`api`]: shared Host operations and bounded responses
- [`job`]: consolidation selection rules and result validation
- [`journal`]: individually committed activity operations, source coverage, retry receipts, and bounded reading
