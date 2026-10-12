# workdir

Provides contracts for persistent Workdir identity and Worker-bound operation sessions. Handles local, HTTP, and External providers through shared scope and capability boundaries. Providers own materialization and deletion; caller Tools do not take over those responsibilities.

## Reference index

- [`WorkdirSession`] — provider operation and close boundaries.
- [`WorkdirSessionRouter`] — aliases, connection generations, and detach coordination.
- [`WorkdirToolBroker`] — scope delegation and write leases for child sessions.
- [`checkout`] — identity-validated operations and search coordinates.
- [`external`] / [`http`] — typed transport, unknown outcomes, and denial reasons.
- [`workspace`] — inventory projection without host information.
