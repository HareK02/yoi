CREATE TABLE passkey_metadata (
    credential_id TEXT PRIMARY KEY REFERENCES passkey_credentials(credential_id) ON DELETE CASCADE,
    key_reference TEXT NOT NULL UNIQUE,
    normal_login_verified_at TEXT
);
INSERT INTO passkey_metadata(credential_id, key_reference)
    SELECT credential_id, lower(hex(randomblob(16))) FROM passkey_credentials;
CREATE TRIGGER passkey_metadata_insert AFTER INSERT ON passkey_credentials
BEGIN
    INSERT INTO passkey_metadata(credential_id, key_reference)
        VALUES (NEW.credential_id, lower(hex(randomblob(16))));
END;
CREATE TABLE passkey_addition_receipts (
    challenge_id TEXT PRIMARY KEY,
    credential_id TEXT NOT NULL UNIQUE REFERENCES passkey_credentials(credential_id) ON DELETE CASCADE,
    user_id TEXT NOT NULL REFERENCES users(user_id) ON DELETE CASCADE,
    authentication_reference TEXT NOT NULL
);
