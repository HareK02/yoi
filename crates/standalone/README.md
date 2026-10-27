# standalone

A Host that starts existing Workers in the same process. Assembles interactive Workers, local persistence, Host-owned Jobs, and explicitly selected Subjects. Does not create Runtime/Workspace registries or a separate execution system.

## Reference index

- [`host`] — bootstrap and confirmed shutdown, with or without a TUI.
- [`unix`] — optional operator transport; disconnect does not release execution ownership.
- [`launch`] — Profile, cwd, and Subject validation before launch.
- [`store`] — Worker records and leases.
- [`jobs`] — Job execution and independent result and cleanup lifetimes.
- [`subjektiv`] — exclusive ownership of local Subjects and bounded capabilities.
