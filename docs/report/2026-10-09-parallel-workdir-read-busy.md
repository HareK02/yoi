# Parallel Workdir reads return transient provider-busy errors

During the Workdir cleanup fix, batches of independent `Read` calls against this checkout repeatedly returned `Workdir session is unavailable: filesystem provider busy` for several calls, while one read in the same batch succeeded. Serial retries against the same paths succeeded. No live Runtime or storage repair was attempted.

This obstructed source inspection and made parallel tool batches less useful. The observations do not establish whether contention is in the filesystem provider, tool broker, or session admission layer.

Suggested investigation: reproduce two concurrent read-only tool calls on one attachment, inspect the admission/lease boundary, and distinguish ordinary read concurrency from destructive-operation exclusion. If temporary rejection is intentional, document which tool combinations may overlap and give a bounded retryable diagnostic. Do not relax cleanup or scope safety to accommodate parallel reads.
