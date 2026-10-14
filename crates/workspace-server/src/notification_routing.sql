-- Source facts commit with the domain write. Policy belongs to Rust, not these triggers.
CREATE TABLE notification_sources (
 sequence INTEGER PRIMARY KEY AUTOINCREMENT,
 workspace_id TEXT NOT NULL, ticket_id TEXT NOT NULL, input_event_id TEXT NOT NULL,
 role TEXT NOT NULL, assignment_id TEXT NOT NULL, kind TEXT NOT NULL,
 actor TEXT, event_index INTEGER, facts_json TEXT NOT NULL CHECK(json_valid(facts_json)),
 UNIQUE(workspace_id,ticket_id,input_event_id,role,assignment_id),
 FOREIGN KEY(workspace_id,ticket_id) REFERENCES typed_tickets(workspace_id,ticket_id)
);
CREATE TABLE notification_decisions (
 source_sequence INTEGER PRIMARY KEY REFERENCES notification_sources(sequence),
 workspace_id TEXT NOT NULL, delivery_id TEXT NOT NULL,
 disposition TEXT NOT NULL CHECK(disposition IN ('deliver','suppress','defer')),
 reason TEXT NOT NULL, purpose TEXT NOT NULL,
 last_offered INTEGER NOT NULL DEFAULT 0,
 UNIQUE(workspace_id,delivery_id)
);
CREATE TABLE notification_requests (
 workspace_id TEXT NOT NULL, delivery_id TEXT NOT NULL, content TEXT NOT NULL,
 target_json TEXT NOT NULL CHECK(json_valid(target_json)),
 PRIMARY KEY(workspace_id,delivery_id),
 FOREIGN KEY(workspace_id,delivery_id) REFERENCES notification_decisions(workspace_id,delivery_id)
);
CREATE TABLE notification_acknowledgements (
 workspace_id TEXT NOT NULL, delivery_id TEXT NOT NULL,
 target_json TEXT NOT NULL CHECK(json_valid(target_json)),
 PRIMARY KEY(workspace_id,delivery_id),
 FOREIGN KEY(workspace_id,delivery_id) REFERENCES notification_requests(workspace_id,delivery_id)
);
CREATE TABLE notification_failures (
 sequence INTEGER PRIMARY KEY AUTOINCREMENT, workspace_id TEXT NOT NULL,
 delivery_id TEXT NOT NULL, code TEXT NOT NULL,
 recorded_at TEXT NOT NULL DEFAULT (strftime('%Y-%m-%dT%H:%M:%fZ','now')),
 FOREIGN KEY(workspace_id,delivery_id) REFERENCES notification_decisions(workspace_id,delivery_id)
);
-- Authorized send targets survive responsibility/binding changes during Runtime I/O.
-- Reservation evidence is sufficient even before rendering; it was authorized under
-- the same write lock. Existing payload/request/ACK rows are never rewritten.
CREATE TABLE notification_assignment_targets (
 workspace_id TEXT NOT NULL, delivery_id TEXT NOT NULL,
 runtime_id TEXT NOT NULL, worker_id TEXT NOT NULL, agent_key TEXT NOT NULL,
 authentication_binding_id TEXT NOT NULL,
 PRIMARY KEY(workspace_id,delivery_id,runtime_id,worker_id,agent_key,authentication_binding_id),
 FOREIGN KEY(workspace_id,delivery_id) REFERENCES ticket_assignment_deliveries(workspace_id,delivery_id)
);
INSERT INTO notification_assignment_targets
 SELECT workspace_id,delivery_id,json_extract(reserved_source_json,'$.runtime_id'),
 json_extract(reserved_source_json,'$.worker_id'),json_extract(reserved_source_json,'$.agent_key'),
 json_extract(reserved_source_json,'$.authentication_binding_id') FROM ticket_assignment_deliveries;
-- A pre-upgrade ACK also proves its target passed the old current/source guards;
-- it may be a replacement different from the immutable original reservation.
INSERT OR IGNORE INTO notification_assignment_targets
 SELECT workspace_id,delivery_id,json_extract(source_json,'$.runtime_id'),
 json_extract(source_json,'$.worker_id'),json_extract(source_json,'$.agent_key'),
 json_extract(source_json,'$.authentication_binding_id') FROM ticket_assignment_delivery_acknowledgements;
CREATE INDEX notification_sources_assignment ON notification_sources(workspace_id,ticket_id,assignment_id,role,sequence);
CREATE INDEX notification_sources_unrouted ON notification_sources(workspace_id,sequence);
CREATE INDEX notification_decisions_pending ON notification_decisions(workspace_id,disposition,last_offered,source_sequence);
CREATE INDEX notification_failures_delivery ON notification_failures(workspace_id,delivery_id,sequence);
-- Retain old content identities and rows; remove notification eligibility from capture.
DROP TRIGGER ticket_assignment_title_input;
DROP TRIGGER ticket_assignment_initial_post_input;
ALTER TABLE ticket_assignment_content_inputs ADD COLUMN actor TEXT;
ALTER TABLE ticket_assignment_content_inputs ADD COLUMN kind TEXT NOT NULL DEFAULT 'content_edit';
ALTER TABLE ticket_assignment_content_inputs ADD COLUMN source_change_id TEXT REFERENCES typed_ticket_content_changes(change_id);
-- Keep the operational input identity while consuming complete domain facts, including editor.
CREATE TRIGGER ticket_assignment_title_input AFTER INSERT ON typed_ticket_content_changes
WHEN NEW.title_changed=1 BEGIN
 INSERT INTO ticket_assignment_content_inputs(workspace_id,ticket_id,assignment_id,after_ticket_event_index,actor,kind,source_change_id)
 SELECT NEW.workspace_id,NEW.ticket_id,c.assignment_id,
 COALESCE((SELECT MAX(event_index) FROM typed_ticket_events WHERE workspace_id=NEW.workspace_id AND ticket_id=NEW.ticket_id),-1),NEW.actor,NEW.kind,NEW.change_id
 FROM ticket_current_worker_assignments c WHERE c.workspace_id=NEW.workspace_id AND c.ticket_id=NEW.ticket_id;
END;
CREATE TRIGGER ticket_assignment_initial_post_input AFTER INSERT ON typed_ticket_content_changes
WHEN NEW.title_changed=0 BEGIN
 INSERT INTO ticket_assignment_content_inputs(workspace_id,ticket_id,assignment_id,after_ticket_event_index,actor,kind,source_change_id)
 SELECT NEW.workspace_id,NEW.ticket_id,c.assignment_id,
 COALESCE((SELECT MAX(event_index) FROM typed_ticket_events WHERE workspace_id=NEW.workspace_id AND ticket_id=NEW.ticket_id),-1),NEW.actor,NEW.kind,NEW.change_id
 FROM ticket_current_worker_assignments c WHERE c.workspace_id=NEW.workspace_id AND c.ticket_id=NEW.ticket_id;
END;
