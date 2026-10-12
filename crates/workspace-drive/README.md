# workspace-drive

Stores the latest Workspace-owned document tree using SQLite metadata and immutable blobs.
Does not provide a POSIX filesystem, document version history, or Worker/Session-owned attachments. The Host handles HTTP authentication and grants.

## Main entry points

- [`Drive`]: document tree, reads, collection, and deletion
- [`Mutation`], [`Drive::mutate`]: updates based on observed mutations
- [`RequestStatus`], [`Drive::request_status`]: committed operation results
