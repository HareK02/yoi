CREATE TABLE content_objects (
    kind INTEGER NOT NULL CHECK(kind IN (1,2,3)),
    hash BLOB NOT NULL CHECK(length(hash)=32),
    bytes BLOB NOT NULL,
    PRIMARY KEY(kind,hash)
) WITHOUT ROWID;
CREATE TABLE content_scopes (
    workspace TEXT NOT NULL,
    target TEXT NOT NULL,
    head BLOB CHECK(head IS NULL OR length(head)=32),
    erased INTEGER NOT NULL DEFAULT 0 CHECK(erased IN (0,1)),
    PRIMARY KEY(workspace,target),
    CHECK(erased=0 OR head IS NULL)
) WITHOUT ROWID;
CREATE TABLE content_scope_objects (
    workspace TEXT NOT NULL,
    target TEXT NOT NULL,
    kind INTEGER NOT NULL,
    hash BLOB NOT NULL,
    PRIMARY KEY(workspace,target,kind,hash),
    FOREIGN KEY(workspace,target) REFERENCES content_scopes(workspace,target),
    FOREIGN KEY(kind,hash) REFERENCES content_objects(kind,hash)
) WITHOUT ROWID;
CREATE TABLE content_reachable (
    workspace TEXT NOT NULL,
    target TEXT NOT NULL,
    kind INTEGER NOT NULL,
    hash BLOB NOT NULL,
    PRIMARY KEY(workspace,target,kind,hash),
    FOREIGN KEY(workspace,target,kind,hash)
        REFERENCES content_scope_objects(workspace,target,kind,hash)
) WITHOUT ROWID;
