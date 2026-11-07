CREATE TABLE workspace_configuration_publication_observations (
    workspace_id TEXT NOT NULL,
    operation_id TEXT NOT NULL,
    stage TEXT NOT NULL CHECK(stage IN ('dispatch','projection','runtime')),
    runtime_id TEXT NOT NULL,
    projection_digest TEXT,
    outcome TEXT NOT NULL CHECK(outcome IN ('observed','unconfirmed')),
    failure_cause TEXT CHECK(failure_cause IS NULL OR
        (typeof(failure_cause)='text' AND length(CAST(failure_cause AS BLOB))<=32768)),
    PRIMARY KEY(workspace_id,operation_id,stage,runtime_id),
    FOREIGN KEY(workspace_id,operation_id) REFERENCES workspace_configuration_receipts(workspace_id,operation_id) ON DELETE RESTRICT,
    CHECK((stage='runtime' AND length(runtime_id)>0 AND projection_digest IS NOT NULL) OR
          (stage!='runtime' AND runtime_id='')),
    CHECK((outcome='observed' AND failure_cause IS NULL) OR
          (outcome='unconfirmed' AND failure_cause IS NOT NULL)),
    CHECK(stage!='dispatch' OR projection_digest IS NULL),
    CHECK(stage!='projection' OR ((outcome='observed')=(projection_digest IS NOT NULL)))
);
CREATE TRIGGER workspace_configuration_publication_no_update BEFORE UPDATE ON workspace_configuration_publication_observations BEGIN
    SELECT RAISE(ABORT, 'configuration publication observations are immutable');
END;
CREATE TRIGGER workspace_configuration_publication_no_delete BEFORE DELETE ON workspace_configuration_publication_observations BEGIN
    SELECT RAISE(ABORT, 'configuration publication observations are retained');
END;
