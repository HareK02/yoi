-- Ticket owns assignment identities, current pointers, audit and operation receipts.
CREATE TABLE IF NOT EXISTS ticket_assignment_operations (
            workspace_id TEXT NOT NULL,
            operation_id TEXT NOT NULL,
            action TEXT NOT NULL CHECK (action IN ('assign', 'reassign', 'unassign')),
            ticket_id TEXT NOT NULL,
            role TEXT NOT NULL DEFAULT 'worker'
                CHECK(role IN ('orchestrator', 'worker', 'owner', 'contributor')),
            principal_kind TEXT NOT NULL DEFAULT 'worker'
                CHECK(principal_kind IN ('user', 'worker', 'workspace_agent')),
            principal_id TEXT,
            runtime_id TEXT,
            worker_id TEXT,
            assignment_id TEXT,
            expected_assignment_id TEXT,
            created_at TEXT NOT NULL,
            request_fingerprint TEXT,
            claim_state TEXT NOT NULL DEFAULT 'committed'
                CHECK(claim_state IN ('pending', 'failed', 'committed')),
            failure_reason TEXT,
            binding_recovery_json TEXT,
            PRIMARY KEY (workspace_id, operation_id),
            FOREIGN KEY (workspace_id) REFERENCES workspaces(workspace_id) ON DELETE CASCADE,
            CHECK(claim_state = 'committed' OR assignment_id IS NULL),
            CHECK((claim_state = 'failed') = (failure_reason IS NOT NULL))
        );
CREATE TABLE IF NOT EXISTS ticket_current_worker_assignments (
            workspace_id TEXT NOT NULL,
            ticket_id TEXT NOT NULL,
            role TEXT NOT NULL DEFAULT 'worker'
                CHECK(role IN ('orchestrator', 'worker', 'owner', 'contributor')),
            assignment_id TEXT NOT NULL,
            principal_kind TEXT NOT NULL DEFAULT 'worker'
                CHECK(principal_kind IN ('user', 'worker', 'workspace_agent')),
            principal_id TEXT,
            runtime_id TEXT,
            worker_id TEXT,
            updated_at TEXT NOT NULL,
            PRIMARY KEY (workspace_id, ticket_id, role, assignment_id),
            UNIQUE (workspace_id, assignment_id),
            FOREIGN KEY (workspace_id, ticket_id, role, assignment_id)
                REFERENCES ticket_worker_assignments(workspace_id, ticket_id, role, assignment_id)
                ON DELETE CASCADE,
            FOREIGN KEY (workspace_id, ticket_id)
                REFERENCES typed_tickets(workspace_id, ticket_id)
                ON DELETE CASCADE,
            CHECK(
                (principal_kind = 'worker' AND runtime_id IS NOT NULL AND worker_id IS NOT NULL AND principal_id IS NULL)
                OR
                (principal_kind != 'worker' AND runtime_id IS NULL AND worker_id IS NULL AND principal_id IS NOT NULL AND length(trim(principal_id)) > 0)
            )
        );
CREATE TABLE IF NOT EXISTS ticket_worker_assignment_events (
            workspace_id TEXT NOT NULL,
            ticket_id TEXT NOT NULL,
            role TEXT NOT NULL DEFAULT 'worker'
                CHECK(role IN ('orchestrator', 'worker', 'owner', 'contributor')),
            event_id TEXT NOT NULL,
            action TEXT NOT NULL CHECK (action IN ('assigned', 'reassigned', 'unassigned')),
            assignment_id TEXT,
            previous_assignment_id TEXT,
            actor TEXT NOT NULL,
            created_at TEXT NOT NULL,
            operation_id TEXT,
            reason TEXT,
            PRIMARY KEY (workspace_id, event_id),
            FOREIGN KEY (workspace_id) REFERENCES workspaces(workspace_id) ON DELETE CASCADE
        );
CREATE TABLE IF NOT EXISTS ticket_worker_assignments (
            workspace_id TEXT NOT NULL,
            ticket_id TEXT NOT NULL,
            assignment_id TEXT NOT NULL,
            role TEXT NOT NULL DEFAULT 'worker'
                CHECK(role IN ('orchestrator', 'worker', 'owner', 'contributor')),
            principal_kind TEXT NOT NULL DEFAULT 'worker'
                CHECK(principal_kind IN ('user', 'worker', 'workspace_agent')),
            principal_id TEXT,
            runtime_id TEXT,
            worker_id TEXT,
            assigned_by TEXT NOT NULL,
            assigned_at TEXT NOT NULL,
            PRIMARY KEY (workspace_id, assignment_id),
            UNIQUE (workspace_id, ticket_id, assignment_id),
            UNIQUE (workspace_id, ticket_id, role, assignment_id),
            UNIQUE (workspace_id, ticket_id, role, assignment_id, principal_kind, principal_id, runtime_id, worker_id),
            FOREIGN KEY (workspace_id) REFERENCES workspaces(workspace_id) ON DELETE CASCADE,
            CHECK(
                (principal_kind = 'worker' AND runtime_id IS NOT NULL AND worker_id IS NOT NULL AND principal_id IS NULL)
                OR
                (principal_kind != 'worker' AND runtime_id IS NULL AND worker_id IS NULL AND principal_id IS NOT NULL AND length(trim(principal_id)) > 0)
            )
        );
CREATE INDEX IF NOT EXISTS idx_ticket_assignment_operations_ticket
            ON ticket_assignment_operations(workspace_id, ticket_id, role, created_at DESC);
CREATE INDEX IF NOT EXISTS idx_ticket_worker_assignment_events_ticket
            ON ticket_worker_assignment_events(workspace_id, ticket_id, role, created_at DESC);
CREATE INDEX IF NOT EXISTS idx_ticket_worker_assignments_principal
            ON ticket_worker_assignments(workspace_id, role, principal_kind, principal_id, runtime_id, worker_id, assigned_at DESC);
CREATE INDEX IF NOT EXISTS idx_ticket_worker_assignments_ticket
            ON ticket_worker_assignments(workspace_id, ticket_id, role, assigned_at DESC);
CREATE INDEX IF NOT EXISTS ticket_current_principal_idx
            ON ticket_current_worker_assignments(workspace_id, principal_kind, principal_id, runtime_id, worker_id);
CREATE UNIQUE INDEX IF NOT EXISTS ticket_current_singleton_role_idx
            ON ticket_current_worker_assignments(workspace_id, ticket_id, role)
            WHERE role IN ('orchestrator', 'worker');
