
CREATE TABLE store_scope (
    singleton INTEGER PRIMARY KEY CHECK (singleton = 1),
    workspace_id TEXT NOT NULL UNIQUE
);

CREATE TABLE subjects (
    subject_id TEXT PRIMARY KEY,
    role TEXT NOT NULL,
    behavior_md TEXT NOT NULL DEFAULT '',
    state TEXT NOT NULL CHECK (state IN ('active', 'retired')),
    record_json TEXT NOT NULL,
    created_at TEXT NOT NULL,
    updated_at TEXT NOT NULL
);

CREATE TABLE staging_records (
    subject_id TEXT NOT NULL,
    candidate_id TEXT NOT NULL,
    kind TEXT NOT NULL CHECK (
        kind IN (
            'preference', 'working_assumption', 'constraint',
            'decision', 'open_question', 'lesson'
        )
    ),
    record_json TEXT NOT NULL,
    created_at TEXT NOT NULL,
    PRIMARY KEY (subject_id, candidate_id),
    FOREIGN KEY (subject_id) REFERENCES subjects(subject_id) ON DELETE RESTRICT
);

CREATE TABLE memory_records (
    subject_id TEXT NOT NULL,
    memory_id TEXT NOT NULL,
    current_change_id TEXT NOT NULL CHECK (length(current_change_id) > 0),
    kind TEXT NOT NULL CHECK (
        kind IN (
            'preference', 'working_assumption', 'constraint',
            'decision', 'open_question', 'lesson'
        )
    ),
    state TEXT NOT NULL CHECK (state IN ('active', 'resolved', 'retracted')),
    record_json TEXT NOT NULL,
    created_at TEXT NOT NULL,
    updated_at TEXT NOT NULL,
    PRIMARY KEY (subject_id, memory_id),
    FOREIGN KEY (subject_id) REFERENCES subjects(subject_id) ON DELETE RESTRICT,
    FOREIGN KEY (subject_id, memory_id, current_change_id)
        REFERENCES memory_changes(subject_id, memory_id, change_id)
        DEFERRABLE INITIALLY DEFERRED
);

CREATE TABLE memory_changes (
    subject_id TEXT NOT NULL,
    memory_id TEXT NOT NULL,
    change_id TEXT NOT NULL CHECK (length(change_id) > 0),
    previous_change_id TEXT,
    kind TEXT NOT NULL CHECK (
        kind IN (
            'preference', 'working_assumption', 'constraint',
            'decision', 'open_question', 'lesson'
        )
    ),
    state TEXT NOT NULL CHECK (state IN ('active', 'resolved', 'retracted')),
    record_json TEXT NOT NULL,
    created_at TEXT NOT NULL,
    PRIMARY KEY (subject_id, memory_id, change_id),
    FOREIGN KEY (subject_id, memory_id)
        REFERENCES memory_records(subject_id, memory_id)
        ON DELETE RESTRICT DEFERRABLE INITIALLY DEFERRED,
    FOREIGN KEY (subject_id, memory_id, previous_change_id)
        REFERENCES memory_changes(subject_id, memory_id, change_id)
        ON DELETE RESTRICT DEFERRABLE INITIALLY DEFERRED,
    CHECK (previous_change_id IS NULL OR previous_change_id <> change_id)
);

CREATE TABLE memory_change_candidates (
    subject_id TEXT NOT NULL,
    memory_id TEXT NOT NULL,
    change_id TEXT NOT NULL CHECK (length(change_id) > 0),
    candidate_id TEXT NOT NULL,
    PRIMARY KEY (subject_id, memory_id, change_id, candidate_id),
    FOREIGN KEY (subject_id, memory_id, change_id)
        REFERENCES memory_changes(subject_id, memory_id, change_id) ON DELETE RESTRICT,
    FOREIGN KEY (subject_id, candidate_id)
        REFERENCES staging_records(subject_id, candidate_id) ON DELETE RESTRICT
);

CREATE TABLE memory_change_derivations (
    subject_id TEXT NOT NULL,
    memory_id TEXT NOT NULL,
    change_id TEXT NOT NULL CHECK (length(change_id) > 0),
    source_memory_id TEXT NOT NULL,
    source_change_id TEXT NOT NULL CHECK (length(source_change_id) > 0),
    PRIMARY KEY (
        subject_id, memory_id, change_id, source_memory_id, source_change_id
    ),
    FOREIGN KEY (subject_id, memory_id, change_id)
        REFERENCES memory_changes(subject_id, memory_id, change_id) ON DELETE RESTRICT,
    FOREIGN KEY (subject_id, source_memory_id, source_change_id)
        REFERENCES memory_changes(subject_id, memory_id, change_id) ON DELETE RESTRICT
);

CREATE TABLE staging_resolutions (
    subject_id TEXT NOT NULL,
    candidate_id TEXT NOT NULL,
    resolution_id TEXT NOT NULL,
    action TEXT NOT NULL CHECK (
        action IN ('applied', 'discarded', 'invalid', 'duplicate', 'already_covered')
    ),
    reason TEXT NOT NULL,
    affected_refs_json TEXT NOT NULL,
    staging_raw_json TEXT NOT NULL,
    resolution_json TEXT NOT NULL,
    resolved_at TEXT NOT NULL,
    PRIMARY KEY (subject_id, candidate_id),
    UNIQUE (resolution_id),
    FOREIGN KEY (subject_id, candidate_id)
        REFERENCES staging_records(subject_id, candidate_id) ON DELETE RESTRICT
);

CREATE TABLE staging_resolution_targets (
    subject_id TEXT NOT NULL,
    candidate_id TEXT NOT NULL,
    memory_id TEXT NOT NULL,
    change_id TEXT NOT NULL CHECK (length(change_id) > 0),
    PRIMARY KEY (subject_id, candidate_id, memory_id, change_id),
    FOREIGN KEY (subject_id, candidate_id)
        REFERENCES staging_resolutions(subject_id, candidate_id) ON DELETE RESTRICT,
    FOREIGN KEY (subject_id, memory_id, change_id)
        REFERENCES memory_changes(subject_id, memory_id, change_id) ON DELETE RESTRICT
);

CREATE TABLE surface_snapshots (
    subject_id TEXT NOT NULL,
    snapshot_id TEXT NOT NULL,
    built_from_memory_fingerprint TEXT NOT NULL,
    snapshot_json TEXT NOT NULL,
    created_at TEXT NOT NULL,
    PRIMARY KEY (subject_id, snapshot_id),
    FOREIGN KEY (subject_id) REFERENCES subjects(subject_id) ON DELETE RESTRICT
);

CREATE TABLE surface_snapshot_refs (
    subject_id TEXT NOT NULL,
    snapshot_id TEXT NOT NULL,
    memory_id TEXT NOT NULL,
    change_id TEXT NOT NULL CHECK (length(change_id) > 0),
    PRIMARY KEY (subject_id, snapshot_id, memory_id),
    FOREIGN KEY (subject_id, snapshot_id)
        REFERENCES surface_snapshots(subject_id, snapshot_id) ON DELETE RESTRICT,
    FOREIGN KEY (subject_id, memory_id, change_id)
        REFERENCES memory_changes(subject_id, memory_id, change_id) ON DELETE RESTRICT
);

CREATE TABLE memory_change_seals (
    subject_id TEXT NOT NULL,
    memory_id TEXT NOT NULL,
    change_id TEXT NOT NULL CHECK (length(change_id) > 0),
    PRIMARY KEY (subject_id, memory_id, change_id),
    FOREIGN KEY (subject_id, memory_id, change_id)
        REFERENCES memory_changes(subject_id, memory_id, change_id) ON DELETE RESTRICT
);

CREATE TABLE staging_resolution_seals (
    subject_id TEXT NOT NULL,
    candidate_id TEXT NOT NULL,
    PRIMARY KEY (subject_id, candidate_id),
    FOREIGN KEY (subject_id, candidate_id)
        REFERENCES staging_resolutions(subject_id, candidate_id) ON DELETE RESTRICT
);

CREATE TABLE surface_snapshot_seals (
    subject_id TEXT NOT NULL,
    snapshot_id TEXT NOT NULL,
    PRIMARY KEY (subject_id, snapshot_id),
    FOREIGN KEY (subject_id, snapshot_id)
        REFERENCES surface_snapshots(subject_id, snapshot_id) ON DELETE RESTRICT
);

CREATE TRIGGER memory_change_candidates_no_late_insert
BEFORE INSERT ON memory_change_candidates
WHEN EXISTS (
    SELECT 1 FROM memory_change_seals
    WHERE subject_id = NEW.subject_id
      AND memory_id = NEW.memory_id
      AND change_id = NEW.change_id
) BEGIN
    SELECT RAISE(ABORT, 'subjektiv change_id evidence is sealed');
END;
CREATE TRIGGER memory_change_derivations_no_late_insert
BEFORE INSERT ON memory_change_derivations
WHEN EXISTS (
    SELECT 1 FROM memory_change_seals
    WHERE subject_id = NEW.subject_id
      AND memory_id = NEW.memory_id
      AND change_id = NEW.change_id
) BEGIN
    SELECT RAISE(ABORT, 'subjektiv derivations are sealed');
END;
CREATE TRIGGER staging_resolution_targets_no_late_insert
BEFORE INSERT ON staging_resolution_targets
WHEN EXISTS (
    SELECT 1 FROM staging_resolution_seals
    WHERE subject_id = NEW.subject_id
      AND candidate_id = NEW.candidate_id
) BEGIN
    SELECT RAISE(ABORT, 'subjektiv resolution targets are sealed');
END;
CREATE TRIGGER surface_snapshot_refs_no_late_insert
BEFORE INSERT ON surface_snapshot_refs
WHEN EXISTS (
    SELECT 1 FROM surface_snapshot_seals
    WHERE subject_id = NEW.subject_id
      AND snapshot_id = NEW.snapshot_id
) BEGIN
    SELECT RAISE(ABORT, 'subjektiv surface references are sealed');
END;

CREATE TRIGGER memory_change_seals_no_update
BEFORE UPDATE ON memory_change_seals BEGIN
    SELECT RAISE(ABORT, 'subjektiv change_id seals are immutable');
END;
CREATE TRIGGER memory_change_seals_no_delete
BEFORE DELETE ON memory_change_seals BEGIN
    SELECT RAISE(ABORT, 'subjektiv change_id seals are retained');
END;
CREATE TRIGGER staging_resolution_seals_no_update
BEFORE UPDATE ON staging_resolution_seals BEGIN
    SELECT RAISE(ABORT, 'subjektiv resolution seals are immutable');
END;
CREATE TRIGGER staging_resolution_seals_no_delete
BEFORE DELETE ON staging_resolution_seals BEGIN
    SELECT RAISE(ABORT, 'subjektiv resolution seals are retained');
END;
CREATE TRIGGER surface_snapshot_seals_no_update
BEFORE UPDATE ON surface_snapshot_seals BEGIN
    SELECT RAISE(ABORT, 'subjektiv snapshot seals are immutable');
END;
CREATE TRIGGER surface_snapshot_seals_no_delete
BEFORE DELETE ON surface_snapshot_seals BEGIN
    SELECT RAISE(ABORT, 'subjektiv snapshot seals are retained');
END;

CREATE TRIGGER memory_changes_no_update
BEFORE UPDATE ON memory_changes BEGIN
    SELECT RAISE(ABORT, 'subjektiv memory changes are immutable');
END;
CREATE TRIGGER memory_changes_no_delete
BEFORE DELETE ON memory_changes BEGIN
    SELECT RAISE(ABORT, 'subjektiv memory changes are retained');
END;
CREATE TRIGGER memory_change_candidates_no_update
BEFORE UPDATE ON memory_change_candidates BEGIN
    SELECT RAISE(ABORT, 'subjektiv change_id evidence is immutable');
END;
CREATE TRIGGER memory_change_candidates_no_delete
BEFORE DELETE ON memory_change_candidates BEGIN
    SELECT RAISE(ABORT, 'subjektiv change_id evidence is retained');
END;
CREATE TRIGGER memory_change_derivations_no_update
BEFORE UPDATE ON memory_change_derivations BEGIN
    SELECT RAISE(ABORT, 'subjektiv derivations are immutable');
END;
CREATE TRIGGER memory_change_derivations_no_delete
BEFORE DELETE ON memory_change_derivations BEGIN
    SELECT RAISE(ABORT, 'subjektiv derivations are retained');
END;
CREATE TRIGGER staging_records_no_update
BEFORE UPDATE ON staging_records BEGIN
    SELECT RAISE(ABORT, 'subjektiv staging records are immutable');
END;
CREATE TRIGGER staging_records_no_delete
BEFORE DELETE ON staging_records BEGIN
    SELECT RAISE(ABORT, 'subjektiv staging records are retained');
END;
CREATE TRIGGER staging_resolutions_no_update
BEFORE UPDATE ON staging_resolutions BEGIN
    SELECT RAISE(ABORT, 'subjektiv staging resolutions are immutable');
END;
CREATE TRIGGER staging_resolutions_no_delete
BEFORE DELETE ON staging_resolutions BEGIN
    SELECT RAISE(ABORT, 'subjektiv staging resolutions are retained');
END;
CREATE TRIGGER staging_resolution_targets_no_update
BEFORE UPDATE ON staging_resolution_targets BEGIN
    SELECT RAISE(ABORT, 'subjektiv resolution targets are immutable');
END;
CREATE TRIGGER staging_resolution_targets_no_delete
BEFORE DELETE ON staging_resolution_targets BEGIN
    SELECT RAISE(ABORT, 'subjektiv resolution targets are retained');
END;
CREATE TRIGGER surface_snapshots_no_update
BEFORE UPDATE ON surface_snapshots BEGIN
    SELECT RAISE(ABORT, 'subjektiv surface snapshots are immutable');
END;
CREATE TRIGGER surface_snapshots_no_delete
BEFORE DELETE ON surface_snapshots BEGIN
    SELECT RAISE(ABORT, 'subjektiv surface snapshots are retained');
END;
CREATE TRIGGER surface_snapshot_refs_no_update
BEFORE UPDATE ON surface_snapshot_refs BEGIN
    SELECT RAISE(ABORT, 'subjektiv surface references are immutable');
END;
CREATE TRIGGER surface_snapshot_refs_no_delete
BEFORE DELETE ON surface_snapshot_refs BEGIN
    SELECT RAISE(ABORT, 'subjektiv surface references are retained');
END;

CREATE TABLE subject_session_attributions (
    session_id TEXT PRIMARY KEY,
    subject_id TEXT NOT NULL,
    runtime_id TEXT NOT NULL,
    worker_id TEXT NOT NULL,
    record_json TEXT NOT NULL,
    attributed_at TEXT NOT NULL,
    FOREIGN KEY (subject_id) REFERENCES subjects(subject_id) ON DELETE RESTRICT
);

CREATE INDEX subject_session_attributions_by_subject
ON subject_session_attributions(subject_id, attributed_at, session_id);

CREATE TRIGGER subject_session_attributions_no_update
BEFORE UPDATE ON subject_session_attributions BEGIN
    SELECT RAISE(ABORT, 'subjektiv session attribution is immutable');
END;
CREATE TRIGGER subject_session_attributions_no_delete
BEFORE DELETE ON subject_session_attributions BEGIN
    SELECT RAISE(ABORT, 'subjektiv session attribution is retained');
END;

CREATE TABLE candidate_decision_receipts (
    subject_id TEXT NOT NULL,
    request_id TEXT NOT NULL,
    candidate_id TEXT NOT NULL,
    request_json TEXT NOT NULL,
    receipt_json TEXT NOT NULL,
    created_at TEXT NOT NULL,
    PRIMARY KEY (subject_id, request_id),
    UNIQUE (subject_id, candidate_id),
    FOREIGN KEY (subject_id, candidate_id)
        REFERENCES staging_resolutions(subject_id, candidate_id) ON DELETE RESTRICT
);

CREATE TRIGGER candidate_decision_receipts_no_update
BEFORE UPDATE ON candidate_decision_receipts BEGIN
    SELECT RAISE(ABORT, 'subjektiv candidate decision receipts are immutable');
END;
CREATE TRIGGER candidate_decision_receipts_no_delete
BEFORE DELETE ON candidate_decision_receipts BEGIN
    SELECT RAISE(ABORT, 'subjektiv candidate decision receipts are retained');
END;

CREATE TABLE surface_generation_state (
    subject_id TEXT PRIMARY KEY,
    memory_fingerprint TEXT NOT NULL,
    status TEXT NOT NULL CHECK (status IN ('dirty', 'failed', 'ready')),
    snapshot_id TEXT,
    reason_code TEXT,
    updated_at TEXT NOT NULL,
    FOREIGN KEY (subject_id) REFERENCES subjects(subject_id) ON DELETE RESTRICT,
    FOREIGN KEY (subject_id, snapshot_id)
        REFERENCES surface_snapshots(subject_id, snapshot_id) ON DELETE RESTRICT
);


CREATE TABLE surface_generation_runs (
    subject_id TEXT NOT NULL,
    generation_id TEXT NOT NULL,
    memory_fingerprint TEXT NOT NULL,
    active_memory_count INTEGER NOT NULL CHECK (active_memory_count >= 0),
    materials_json TEXT NOT NULL,
    job_id TEXT,
    attempt_id TEXT,
    created_at TEXT NOT NULL,
    PRIMARY KEY (subject_id, generation_id),
    FOREIGN KEY (subject_id) REFERENCES subjects(subject_id) ON DELETE RESTRICT
);

CREATE INDEX surface_generation_runs_by_change
ON surface_generation_runs(subject_id, memory_fingerprint, created_at, generation_id);

CREATE TRIGGER surface_generation_runs_no_update
BEFORE UPDATE ON surface_generation_runs BEGIN
    SELECT RAISE(ABORT, 'subjektiv surface generation inputs are immutable');
END;
CREATE TRIGGER surface_generation_runs_no_delete
BEFORE DELETE ON surface_generation_runs BEGIN
    SELECT RAISE(ABORT, 'subjektiv surface generation inputs are retained');
END;
