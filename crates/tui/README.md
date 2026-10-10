# tui

## Role

`tui` implements terminal UI clients for the single-Worker Console and workspace Dashboard surfaces.

## Boundaries

Owns:

- terminal rendering and input handling
- local composer state and UI affordances
- single-Worker Console attach/restore/chat screens
- workspace Dashboard presentation and role-action UI

Does not own:

- durable transcript authority (`session-store`)
- Worker current state (`session-store` worker metadata)
- Worker lifecycle policy (`worker`)
- product CLI ownership (`yoi`)

## Design notes

The TUI should display committed events and Worker snapshots rather than inventing durable state. Local input history and optimistic UI affordances are editing conveniences; they must not become hidden model context.

## See also

- [`../../docs/design/context-history.md`](../../docs/design/context-history.md)
- [`../../docs/design/worker-session-state.md`](../../docs/design/worker-session-state.md)

## Feature invocation editing

`/` discovers enabled Feature declarations with descriptions and usage. Choose a
name or alias with Tab/Enter, complete its parenthesized arguments, and press
Enter to confirm that selected occurrence as a typed chip. A later Enter sends
the message. Raw slash text and paste content are never dispatched as commands.
Use Alt+Enter beside a Feature chip to rediscover its declaration and reopen its
arguments; Backspace/Delete removes a chip atomically. Typed chips survive local
history, draft restoration, transport retry, and rewind.

Every completion and chip-re-edit query carries a fresh `request_id`. Replies
must echo that ID and match the active query context and Worker target. The first
input/cursor edit cancels the pending input watch; returning to identical text
cannot revive it. Authority snapshots and Worker/view switches discard pending
queries and visible candidates, rather than advancing another counter.
Uncorrelated replies are ignored, even for File queries where the wire field
remains optional for legacy callers.

The declared attachment adapter stages a client-local file in place, replacing
its staging chip with an `UploadedFile` reference when ready. Local file argument
completion runs on the client, not Worker. Alt+Enter beside a failed staging chip
retries with the same upload ID; deleting an unsent chip cancels only its owned
staging resource. Accepted, restored, or history attachments are never deleted by
composer cleanup. Staging blocks Send/Notify and history navigation until it is
ready, retried, or removed. There is no raw `/attach` or `/clear-attachments`
command lane.

Attachment staging remains Backend-only: the existing Standalone transport has
no client-local file-upload staging API. The TUI reports that limitation and
preserves the draft invocation; it never falls back to raw path dispatch.
