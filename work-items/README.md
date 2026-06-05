# Legacy work-items notice

Active Yoi Ticket storage has moved to `.yoi/tickets/`.

This directory is intentionally not a live mutable backend. It remains only as a compatibility notice for older references and migration history. Do not create `open/`, `pending/`, or `closed/` Ticket records here.

Use `yoi ticket ...` or Ticket tools; both operate on the active `.yoi/tickets/` storage.
