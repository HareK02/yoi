CREATE TABLE external_workdir_connection_observations (
    observation_id INTEGER PRIMARY KEY,
    workspace_id TEXT NOT NULL,
    grant_id TEXT NOT NULL,
    provider_instance_id TEXT NOT NULL,
    grant_generation INTEGER NOT NULL CHECK (grant_generation > 0),
    connection_generation INTEGER NOT NULL CHECK (connection_generation > 0),
    closed_reason TEXT NOT NULL CHECK (closed_reason IN (
        'activation_failed', 'registration_send_failed', 'heartbeat_timeout',
        'send_failed', 'command_channel_closed', 'invalid_frame', 'peer_closed',
        'revoke_acknowledged'
    )),
    failures_json TEXT CHECK (failures_json IS NULL OR length(CAST(failures_json AS BLOB)) <= 262144),
    observed_at TEXT NOT NULL,
    CHECK (connection_generation = grant_generation OR
        (connection_generation = grant_generation + 1 AND closed_reason = 'activation_failed')),
    FOREIGN KEY (workspace_id, grant_id)
        REFERENCES external_workdir_grants(workspace_id, grant_id) ON DELETE CASCADE
);
CREATE INDEX external_workdir_connection_observations_owner
    ON external_workdir_connection_observations(workspace_id, grant_id, grant_generation, observation_id);
CREATE TRIGGER external_workdir_connection_observations_no_update
BEFORE UPDATE ON external_workdir_connection_observations
BEGIN
    SELECT RAISE(ABORT, 'connection observations are immutable');
END;
