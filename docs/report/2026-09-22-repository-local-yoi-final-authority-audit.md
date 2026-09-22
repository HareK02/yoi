# T-452 final repository-local `.yoi` authority audit

Date: 2026-09-22
Objective: O-12
Ticket: T-452

## Baseline and method

The implementation branch was created from `develop` at `043cb7bf22bd59d1d553e3f5a7c86c20502fad30` after confirming that ref was the then-current provider-resolved `origin/develop`. This audit follows normal entrypoints through constructors and typed backends to their I/O boundary. A raw `.yoi` string search was used only to enumerate candidates; strings in comments, negative regression fixtures, explicit artifacts, and preserved storage boundaries are not treated as authority by themselves.

The final pre-review pass must refresh from `develop` after T-629 is integrated. T-629 owns conversion of the existing panel E2E setup; this change neither restores nor duplicates that legacy fixture.

## Reachability and disposition

| Surface | Entry point and pre-change path | Disposition |
| --- | --- | --- |
| Ticket config | `TicketConfig::load_workspace` read `<workspace>/.yoi/workspace.toml`, fell back to `.yoi/ticket.config.toml`, and defaulted to `.yoi/tickets`. | Removed the filesystem loader, backend path/provider types, fallback parser, and path resolution. `ticket::config` now contains only filesystem-independent fixed-role domain types. |
| Ticket provider | Public `LocalTicketBackend` performed Ticket tree reads/writes. `SqliteTicketBackend::import_from_local_backend` converted that tree into SQLite. | Removed both production providers/helpers and their filesystem-only tests. SQLite fixtures now seed `typed_tickets`/events directly. Deliberately stale Ticket/Objective trees remain only as ignored-input regressions. |
| Worker Ticket tools | `TicketFeatureBackend::Local`, `TicketFeature::for_workspace[_with_access]`, and local constructors could install filesystem-backed tools even though the normal controller already selected a Workspace client. | Removed the local variant and constructors. The feature now accepts only an injected `Arc<dyn WorkspaceClient>` and constructs `WorkspaceHttpTicketBackend`; there is no provider-selection enum or filesystem path parameter. |
| Memory CLI | `yoi memory lint` -> `memory_lint::run` -> cwd/`--workspace` -> `WorkspaceLayout` -> filesystem linter. | Removed the mode, parser/help branch, module, and CLI dependency. Old invocations fail as unknown before target/cwd resolution and do not read or mutate malformed legacy input. No replacement Memory UX or compatibility shim was added. |
| Memory provider | `WorkspaceLayout::resolve` searched cwd/ancestors and supplied `.yoi/memory` paths to query/resident, staging, consolidation, audit, usage, scope, and lint implementations. `execute_memory_backend_operation` dispatched operations to that layout. | Removed layout/discovery, local executor, resident/scope/usage writers, extraction/consolidation staging files, filesystem audit implementation, and path-scanning linter modules. Kept Backend operation DTOs, extraction payload/tool contracts, audit event DTOs, provenance/frontmatter schema helpers, and lint-common slug types because they are filesystem-independent shared domain/API types. Workspace Server SQLite authority remains the Memory persistence executor. |
| Runtime store | Public `WorkerRuntimeExecutionBackend::from_workspace` derived `<workspace>/.yoi/runtime-store`. | Removed. Runtime callers continue to pass explicit Runtime-owned store/config paths; no repository fallback was introduced. |
| Workspace/client/bootstrap | Normal `yoi init`, client target resolution, Workspace catalog/metadata, Server bootstrap, and repository mapping were already cut over by T-468. | Preserved. Existing root/ancestor stale-marker and clean-init tests continue to assert Server DB/API plus XDG client routing, with no repository write. |
| Profile/Skill/Prompt | Workspace projections were already sourced from the Server virtual config tree; direct Worker startup uses explicit manifest/builtin authority. | Preserved. Existing malformed local profile/prompt/skill fixtures are negative tests. A clean direct Worker-startup regression now verifies that manifest resolution does not create `.yoi`. |
| Plugin | T-603 already removed ambient `.yoi/plugins` discovery. | Preserved the source assertions and explicit `.yoi-plugin` offline package operations. No install/runtime fallback was added. |
| Objective | Worker Objective tools already require Workspace API authority; Workspace Server stores Objective rows in SQLite. | Preserved. Filesystem-only Objective fixtures are ignored-input regressions, not import setup. |
| Web/TUI | Both consume Workspace HTTP/client models and do not construct repository Ticket/Memory backends. | Preserved. Their checks validate the current typed API/routing surfaces; T-629 separately owns the panel E2E fixture modernization. |

## Regression evidence added or strengthened

- Source assertions reject the concrete production provider/constructor chain rather than banning every `.yoi` string: Ticket config/provider/import, Worker local Ticket feature, Memory layout/executor/staging entrypoints (including every compiled extraction implementation module), Runtime `from_workspace`, and CLI `memory_lint` mode.
- `TicketFeature` has no repository/cwd input: installation accepts only an authority-scoped Workspace client and access policy. A regression verifies that separately present malformed repository-root Ticket input and malformed ancestor Workspace input remain byte-for-byte unchanged while the Workspace tool surface installs.
- The removed Memory CLI is unknown with bare, `lint`, help, and legacy `--workspace` forms; a malformed `.yoi/memory` fixture remains byte-for-byte unchanged.
- Direct Worker manifest resolution against a clean explicit workspace does not create `.yoi`. A Runtime integration regression additionally creates a Worker from a clean Git repository, executes a turn, stops/restores the Worker, and verifies that launch and restore never create repository-local `.yoi` authority.
- Ticket tool mutation uses a direct SQLite backend and asserts `tickets.db` is written while `.yoi`/`tickets` are not.
- Workspace Server authority seeds SQLite Ticket/Objectives directly, proves filesystem-only records are ignored, and proves legacy Memory staging cannot enter the Backend consolidation backlog.

## Remaining `.yoi` classifications

The post-change source inventory contains the following intentional classes:

1. **Negative regression input or assertions:** stale/malformed Workspace, Ticket, Objective, Memory, Profile, Skill, Prompt, and Plugin trees; assertions that clean operation creates no `.yoi`; leak-prevention fixtures.
2. **Preserved runtime/server boundaries:** Runtime-owned Workdirs and run artifacts, explicit Server data/config directories, session-store compatibility for the legacy home-directory secret/session boundary, and generated ignore files that prevent execution artifacts from entering Git.
3. **Explicit artifacts:** repository/Git files selected by the user and `.yoi-plugin` package paths passed directly to offline `new`/`check`/`pack` commands.
4. **Documentation/history:** current guidance describing removal and historical reports retained as historical evidence.

None of these classes performs cwd/ancestor authority discovery, automatic import, dual read, or repository-local fallback. The repository checkout and Git metadata remain ordinary filesystem inputs; removing filesystem use in general is outside T-452.

## Upgrade contract

There is no automatic migration, startup import, compatibility reader, or delete pass. Deployments that still need legacy repository-local Ticket or Memory data must export it with the old version before upgrading and import it through a supported Workspace-backed surface. Stale trees are ignored and can be archived or removed only after operator verification.
