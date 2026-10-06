# Command-output readback during T-702

During a cold `cargo test -p yoi-workspace-server` run, Bash bounded its inline output and reported a spill file under the Worker's declared readable `bash-output` boundary. A subsequent `Grep` of that exact returned absolute path was rejected as outside allowed scope. I did not attempt another path to bypass the rejection; instead I reran the relevant tests with bounded output filtering.

This made a timed-out crate test's failure details unavailable through the advertised readback path. Suggested improvement: issue a readback capability/reference for Bash spills usable by Read/Grep, or ensure the Workdir attachment routes the declared readable spill directory consistently. Include the permitted tool/attachment in the spill message if it differs from the repository attachment.

The first whole-crate test also reached the provider's 600-second timeout while an outdated WebSocket fixture awaited a subscription that was rejected by the new operation-time authentication. After providing genuine fixture credentials, the targeted subscription test completed; subsequent whole-crate runs use bounded test concurrency. A timeout was treated as incomplete validation, not a passing suite.
