Workspace Orchestrator attention: authoritative Ticket state still contains queued work after the previous turn or after Server recovery.

Workspace: {{workspace_id}}
Remaining queued Tickets (bounded):
{{ticket_lines}}
{{omitted_line}}
Reread the listed Tickets, their relations, orchestration plans, current assignments, Workers, and Workdirs before acting. Continue only work already authorized by the human `ready -> queued` transition. Do not drain the queue automatically and do not create duplicate assignments, Workers, Workdirs, or merges. If no Ticket is currently actionable, record the durable waiting reason on the authoritative Ticket or orchestration plan and stop. For an actionable queued Ticket, call the guarded `SpawnTicketCoder` operation without first changing Ticket state; it records `queued -> inprogress` only after the Coder, initial input, current assignment, and Workdir finalization are durably accepted.
