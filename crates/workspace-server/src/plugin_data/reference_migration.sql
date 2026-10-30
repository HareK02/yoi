-- New ordered migration only. Never modify the published catalog baseline.
DROP TRIGGER plugin_reference_live;
DROP TRIGGER plugin_uninstall_referenced;
CREATE TABLE plugin_references_with_data (
    workspace_id TEXT NOT NULL,
    kind TEXT NOT NULL CHECK(kind IN ('config','pending','active','restorable','data')),
    owner TEXT NOT NULL CHECK(length(owner) BETWEEN 1 AND 512),
    installation_id TEXT NOT NULL,
    artifact_digest TEXT NOT NULL,
    PRIMARY KEY(workspace_id, kind, owner, installation_id, artifact_digest),
    FOREIGN KEY(workspace_id, installation_id, artifact_digest) REFERENCES plugin_artifacts(workspace_id, installation_id, artifact_digest) ON DELETE RESTRICT
);
INSERT INTO plugin_references_with_data SELECT * FROM plugin_references;
DROP TABLE plugin_references;
ALTER TABLE plugin_references_with_data RENAME TO plugin_references;
CREATE TRIGGER plugin_reference_live BEFORE INSERT ON plugin_references
WHEN NOT EXISTS(SELECT 1 FROM plugin_artifacts a JOIN plugin_installations i USING(workspace_id, installation_id)
 WHERE a.workspace_id=NEW.workspace_id AND a.installation_id=NEW.installation_id AND a.artifact_digest=NEW.artifact_digest AND a.revoked=0 AND i.uninstalled=0)
BEGIN SELECT RAISE(ABORT,'plugin reference target unavailable'); END;
CREATE TRIGGER plugin_uninstall_referenced BEFORE UPDATE OF uninstalled ON plugin_installations
WHEN NEW.uninstalled=1 AND EXISTS(SELECT 1 FROM plugin_references r WHERE r.workspace_id=NEW.workspace_id AND r.installation_id=NEW.installation_id)
BEGIN SELECT RAISE(ABORT,'plugin installation referenced'); END;
