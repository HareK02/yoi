ALTER TABLE worker_create_reservations ADD COLUMN startup_failure_json TEXT
    CHECK (startup_failure_json IS NULL OR
        (typeof(startup_failure_json) = 'text' AND length(CAST(startup_failure_json AS BLOB)) <= 32768));
