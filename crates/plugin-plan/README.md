# Portable Plugin execution plan

This crate owns the immutable [`WorkerPluginExecutionPlan`] model, validation and canonical fingerprint shared by Server persistence and Runtime launch snapshots. [`ArtifactDescriptor`] projects only the artifact metadata needed to construct or compare exact pins without depending on Server API types.

It does not own catalog admission, persistence, configuration resolution, artifact delivery or authorization. A deserialized plan or owner-supplied artifact projection is not evidence of trust or live execution authority; callers must enforce those boundaries with their owners.
