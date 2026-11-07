CREATE TABLE plugin_installations (
    workspace_id TEXT NOT NULL REFERENCES workspaces(workspace_id) ON DELETE CASCADE,
    installation_id TEXT NOT NULL,
    alias TEXT NOT NULL CHECK(length(alias) BETWEEN 1 AND 128),
    package_id TEXT NOT NULL,
    last_operation TEXT NOT NULL,
    uninstalled INTEGER NOT NULL DEFAULT 0 CHECK(uninstalled IN (0,1)),
    PRIMARY KEY(workspace_id, installation_id)
);
CREATE UNIQUE INDEX plugin_live_alias ON plugin_installations(workspace_id, alias) WHERE uninstalled=0;
CREATE TABLE plugin_artifacts (
    workspace_id TEXT NOT NULL,
    installation_id TEXT NOT NULL,
    artifact_digest TEXT NOT NULL CHECK(length(artifact_digest)=71 AND substr(artifact_digest,1,7)='sha256:' AND substr(artifact_digest,8) NOT GLOB '*[^0-9a-f]*'),
    package_digest TEXT NOT NULL CHECK(length(package_digest)=71 AND substr(package_digest,1,7)='sha256:' AND substr(package_digest,8) NOT GLOB '*[^0-9a-f]*'),
    version TEXT NOT NULL,
    artifact_size INTEGER NOT NULL CHECK(artifact_size > 0),
    metadata_json TEXT NOT NULL CHECK(json_valid(metadata_json)),
    revoked INTEGER NOT NULL DEFAULT 0 CHECK(revoked IN (0,1)),
    revocation_reason TEXT,
    CHECK((revoked=0 AND revocation_reason IS NULL) OR (revoked=1 AND revocation_reason IS NOT NULL)),
    PRIMARY KEY(workspace_id, installation_id, artifact_digest),
    UNIQUE(workspace_id, installation_id, version),
    FOREIGN KEY(workspace_id, installation_id) REFERENCES plugin_installations(workspace_id, installation_id) ON DELETE CASCADE
);
CREATE TABLE plugin_uploads (
    workspace_id TEXT NOT NULL REFERENCES workspaces(workspace_id) ON DELETE CASCADE,
    upload_id TEXT NOT NULL,
    actor TEXT NOT NULL,
    expires_at INTEGER NOT NULL,
    artifact_digest TEXT NOT NULL CHECK(length(artifact_digest)=71 AND substr(artifact_digest,1,7)='sha256:' AND substr(artifact_digest,8) NOT GLOB '*[^0-9a-f]*'),
    artifact_size INTEGER NOT NULL CHECK(artifact_size > 0),
    metadata_json TEXT NOT NULL CHECK(json_valid(metadata_json)),
    PRIMARY KEY(workspace_id, upload_id)
);
CREATE INDEX plugin_upload_expiry ON plugin_uploads(expires_at);
CREATE TABLE plugin_operations (
    workspace_id TEXT NOT NULL REFERENCES workspaces(workspace_id) ON DELETE CASCADE,
    operation_id TEXT NOT NULL,
    actor TEXT NOT NULL,
    kind TEXT NOT NULL CHECK(kind IN ('install','add_artifact','revoke','uninstall')),
    operation_key TEXT NOT NULL CHECK(length(operation_key) BETWEEN 1 AND 128),
    fingerprint TEXT NOT NULL CHECK(length(fingerprint)=64),
    result_json TEXT NOT NULL CHECK(json_valid(result_json)),
    PRIMARY KEY(workspace_id, operation_id),
    UNIQUE(workspace_id, actor, kind, operation_key)
);
CREATE TABLE plugin_references (
    workspace_id TEXT NOT NULL,
    kind TEXT NOT NULL CHECK(kind IN ('config','pending','active','restorable')),
    owner TEXT NOT NULL CHECK(length(owner) BETWEEN 1 AND 512),
    installation_id TEXT NOT NULL,
    artifact_digest TEXT NOT NULL,
    PRIMARY KEY(workspace_id, kind, owner, installation_id, artifact_digest),
    FOREIGN KEY(workspace_id, installation_id, artifact_digest) REFERENCES plugin_artifacts(workspace_id, installation_id, artifact_digest) ON DELETE RESTRICT
);
CREATE TRIGGER plugin_revocation_irreversible BEFORE UPDATE OF revoked ON plugin_artifacts
WHEN OLD.revoked=1 AND NEW.revoked<>1 BEGIN SELECT RAISE(ABORT,'plugin revocation is irreversible'); END;
CREATE TRIGGER plugin_reference_live BEFORE INSERT ON plugin_references
WHEN NOT EXISTS(SELECT 1 FROM plugin_artifacts a JOIN plugin_installations i USING(workspace_id, installation_id)
 WHERE a.workspace_id=NEW.workspace_id AND a.installation_id=NEW.installation_id AND a.artifact_digest=NEW.artifact_digest AND a.revoked=0 AND i.uninstalled=0)
BEGIN SELECT RAISE(ABORT,'plugin reference target unavailable'); END;
CREATE TRIGGER plugin_uninstall_referenced BEFORE UPDATE OF uninstalled ON plugin_installations
WHEN NEW.uninstalled=1 AND EXISTS(SELECT 1 FROM plugin_references r WHERE r.workspace_id=NEW.workspace_id AND r.installation_id=NEW.installation_id)
BEGIN SELECT RAISE(ABORT,'plugin installation referenced'); END;
