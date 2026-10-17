-- References outlive explicit Journal erasure. Do not add target-row foreign keys:
-- lifecycle erasure deletes Journal content but retains its Session tombstone.
CREATE TABLE memory_journal_links (
    subject_id TEXT NOT NULL,
    candidate_id TEXT NOT NULL,
    source_index INTEGER NOT NULL CHECK(source_index >= 0),
    reason TEXT NOT NULL CHECK(reason IN (
        'insufficient_source', 'source_not_admitted', 'ambiguous_source',
        'journal_not_fixed', 'linked'
    )),
    journal_id TEXT,
    entry_id TEXT,
    source_session_id TEXT,
    source_order INTEGER,
    PRIMARY KEY(subject_id, candidate_id, source_index),
    FOREIGN KEY(subject_id, candidate_id)
        REFERENCES staging_records(subject_id, candidate_id) ON DELETE RESTRICT,
    CHECK((reason = 'linked' AND journal_id IS NOT NULL AND entry_id IS NOT NULL
            AND source_session_id IS NOT NULL AND source_order IS NOT NULL AND source_order >= 0)
        OR (reason <> 'linked' AND journal_id IS NULL AND entry_id IS NULL
            AND source_session_id IS NULL AND source_order IS NULL))
);
CREATE INDEX memory_journal_links_reverse
    ON memory_journal_links(subject_id, journal_id, entry_id, candidate_id, source_index);
CREATE TRIGGER memory_journal_links_identity BEFORE UPDATE ON memory_journal_links
    WHEN OLD.subject_id <> NEW.subject_id OR OLD.candidate_id <> NEW.candidate_id
        OR OLD.source_index <> NEW.source_index
    BEGIN SELECT RAISE(ABORT, 'Journal source reference identity is immutable'); END;
CREATE TRIGGER memory_journal_links_connected_immutable BEFORE UPDATE ON memory_journal_links
    WHEN OLD.reason = 'linked'
    BEGIN SELECT RAISE(ABORT, 'connected Journal references are immutable'); END;
CREATE TRIGGER memory_journal_links_retained BEFORE DELETE ON memory_journal_links
    BEGIN SELECT RAISE(ABORT, 'Journal references are retained'); END;
