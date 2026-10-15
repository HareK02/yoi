CREATE TABLE journals (
    id TEXT PRIMARY KEY,
    subject_id TEXT NOT NULL REFERENCES subjects(subject_id) ON DELETE RESTRICT,
    source_session_id TEXT NOT NULL UNIQUE,
    title TEXT NOT NULL,
    current_position TEXT NOT NULL,
    process_order INTEGER NOT NULL DEFAULT 0 CHECK (process_order >= 0),
    process_byte INTEGER NOT NULL DEFAULT 0 CHECK (process_byte >= 0),
    state_token TEXT NOT NULL,
    created_at TEXT NOT NULL,
    updated_at TEXT NOT NULL,
    UNIQUE(subject_id, id)
);
CREATE TABLE journal_entries (
    id TEXT PRIMARY KEY,
    subject_id TEXT NOT NULL,
    journal_id TEXT NOT NULL,
    position INTEGER NOT NULL CHECK (position >= 0),
    state TEXT NOT NULL CHECK (state IN ('open', 'fixed')),
    title TEXT NOT NULL,
    body TEXT NOT NULL,
    ongoing TEXT NOT NULL,
    source_end_json TEXT,
    source_order INTEGER,
    created_at TEXT NOT NULL,
    updated_at TEXT NOT NULL,
    fixed_at TEXT,
    UNIQUE(journal_id, position),
    UNIQUE(subject_id, id),
    FOREIGN KEY(subject_id, journal_id) REFERENCES journals(subject_id, id) ON DELETE RESTRICT,
    CHECK ((state = 'open' AND source_end_json IS NULL AND source_order IS NULL AND fixed_at IS NULL)
        OR (state = 'fixed' AND source_end_json IS NOT NULL AND source_order IS NOT NULL AND source_order >= 0 AND fixed_at IS NOT NULL AND ongoing = ''))
);
CREATE UNIQUE INDEX journal_one_open ON journal_entries(journal_id) WHERE state = 'open';
CREATE TRIGGER journal_append_after_open BEFORE INSERT ON journal_entries
WHEN EXISTS (SELECT 1 FROM journal_entries WHERE journal_id = NEW.journal_id AND state = 'open')
BEGIN SELECT RAISE(ABORT, 'journal open must be the tail'); END;
CREATE TRIGGER journal_fixed_immutable BEFORE UPDATE ON journal_entries WHEN OLD.state = 'fixed'
BEGIN SELECT RAISE(ABORT, 'journal fixed entries are immutable'); END;
CREATE TRIGGER journal_entry_identity BEFORE UPDATE ON journal_entries
WHEN OLD.id <> NEW.id OR OLD.subject_id <> NEW.subject_id OR OLD.journal_id <> NEW.journal_id
    OR OLD.position <> NEW.position OR OLD.created_at <> NEW.created_at
BEGIN SELECT RAISE(ABORT, 'journal entry identity is immutable'); END;
CREATE TABLE journal_corrections (
    subject_id TEXT NOT NULL,
    entry_id TEXT NOT NULL,
    target_id TEXT NOT NULL,
    PRIMARY KEY(entry_id, target_id),
    FOREIGN KEY(subject_id, entry_id) REFERENCES journal_entries(subject_id, id) ON DELETE RESTRICT,
    FOREIGN KEY(subject_id, target_id) REFERENCES journal_entries(subject_id, id) ON DELETE RESTRICT,
    CHECK(entry_id <> target_id)
);
CREATE TABLE journal_sources (
    journal_id TEXT NOT NULL REFERENCES journals(id) ON DELETE RESTRICT,
    source_order INTEGER NOT NULL CHECK(source_order >= 0),
    pointer_json TEXT NOT NULL,
    entry_ref TEXT NOT NULL,
    source_json TEXT NOT NULL,
    accepted_json TEXT NOT NULL,
    unavailable_json TEXT NOT NULL,
    reflected_bytes INTEGER NOT NULL DEFAULT 0 CHECK(reflected_bytes >= 0),
    PRIMARY KEY(journal_id, source_order),
    UNIQUE(journal_id, pointer_json),
    UNIQUE(journal_id, entry_ref)
);
CREATE TABLE journal_generations (
    id TEXT PRIMARY KEY,
    journal_id TEXT NOT NULL REFERENCES journals(id) ON DELETE RESTRICT,
    job_id TEXT NOT NULL,
    attempt_id TEXT NOT NULL,
    request_json TEXT NOT NULL,
    snapshot_json TEXT NOT NULL,
    active INTEGER NOT NULL CHECK(active IN (0, 1)),
    UNIQUE(journal_id, job_id, attempt_id)
);
CREATE UNIQUE INDEX journal_one_generation ON journal_generations(journal_id) WHERE active = 1;
CREATE TABLE journal_operations (
    journal_id TEXT NOT NULL REFERENCES journals(id) ON DELETE RESTRICT,
    request_id TEXT NOT NULL,
    generation_id TEXT NOT NULL REFERENCES journal_generations(id) ON DELETE RESTRICT,
    request_json TEXT NOT NULL,
    receipt_json TEXT NOT NULL,
    PRIMARY KEY(journal_id, request_id)
);
CREATE TRIGGER journal_operations_no_update BEFORE UPDATE ON journal_operations
BEGIN SELECT RAISE(ABORT, 'journal operation receipts are immutable'); END;
CREATE TRIGGER journal_operations_no_delete BEFORE DELETE ON journal_operations
BEGIN SELECT RAISE(ABORT, 'journal operation receipts are retained'); END;
