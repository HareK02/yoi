CREATE TABLE passkey_addition_codes (
    user_id TEXT PRIMARY KEY REFERENCES users(user_id) ON DELETE CASCADE,
    issuance_id TEXT NOT NULL UNIQUE,
    code_hash TEXT NOT NULL,
    target_origin TEXT NOT NULL,
    rp_id TEXT NOT NULL,
    expires_at TEXT NOT NULL,
    failed_attempts INTEGER NOT NULL DEFAULT 0 CHECK (failed_attempts BETWEEN 0 AND 5)
);
