# Legacy work-items notice

Active Yoi Ticket storage has moved to `.yoi/tickets/`.

This directory is intentionally not a live mutable backend. It remains only as a compatibility notice for older references and migration history. Do not create `open/`, `pending/`, or `closed/` Ticket records here.

Use `yoi ticket ...`, Ticket tools, or the transitional `./tickets.sh` shim from the repository root; all default to `.yoi/tickets/`.
