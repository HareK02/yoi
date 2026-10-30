// Generated from server-api. Do not edit by hand.
// Regenerate: cargo run -q -p server-api --features typescript --example generate_plugin_api_types > web/workspace/src/lib/generated/plugin-api.ts

export type PluginInstallRequest = { upload_id: string, alias: string, operation_key: string, };

export type PluginAddArtifactRequest = { upload_id: string, expected_last_operation: string, operation_key: string, };

export type PluginRevokeRequest = { artifact_digest: string, expected_last_operation: string, reason: string, operation_key: string, };

export type PluginUninstallRequest = { expected_last_operation: string, operation_key: string, };

export type PluginListRequest = { cursor: string | null, limit: number | null, };

export type PluginSchemaContribution = { identity: string, version: string, digest: string, namespace: string, dcdl: string, };

export type PluginToolMetadata = { name: string, description: string, input_schema: unknown, output_schema: unknown, error_codes: Array<string>, external_write: boolean, };

export type PluginHttpMethod = "GET" | "HEAD" | "POST" | "PUT" | "PATCH" | "DELETE" | "OPTIONS";

export type PluginConnectionMetadata = { name: string, methods: Array<PluginHttpMethod>, path_prefixes: Array<string>, };

export type PluginGrantMetadata = { "kind": "logging" } | { "kind": "https", slot: string, } | { "kind": "ticket_read" } | { "kind": "ticket_events" } | { "kind": "ticket_data_read", collection: string, schema_id: string, schema_version: string, } | { "kind": "ticket_data_write", collection: string, schema_id: string, schema_version: string, };

export type PluginServiceMetadata = { name: string, subscriptions: Array<string>, };

export type PluginServicePauseReason = "rejected" | "attempts_exhausted" | "queue_full" | "retention_gap" | "indeterminate" | "authorization";

export type PluginServiceDiagnostics = { consumer_id: string, pause_reason: PluginServicePauseReason | null, queued: number, attempt: number, current_event_id: string | null, lease_active: boolean, };

export type PluginServiceRecoveryAction = { "kind": "resume" } | { "kind": "skip", event_id: string, } | { "kind": "accept_gap" };

export type PluginServiceRecoveryRequest = { consumer_id: string, expected_pause: PluginServicePauseReason, action: PluginServiceRecoveryAction, operation_key: string, reason: string, };

export type PluginDataSchemaMetadata = { collection: string, schema_id: string, schema_version: string, schema: unknown, };

export type PluginDataPurgeState = { installation_last_operation: string, cells_digest: string, cell_count: number, };

export type PluginDataPurgeRequest = { operation_key: string, expected: PluginDataPurgeState, };

export type PluginDataPurgeResponse = { purge_id: string, observed: PluginDataPurgeState, };

export type PluginArtifact = { package_id: string, name: string, version: string, description: string | null, package_digest: string, artifact_digest: string, artifact_size: number, schema_version: number, world: string, required_imports: Array<string>, tools: Array<PluginToolMetadata>, services?: Array<PluginServiceMetadata>, connections: Array<PluginConnectionMetadata>, requested_grants: Array<PluginGrantMetadata>, data_collections: Array<PluginDataSchemaMetadata>, config_schema: PluginSchemaContribution, revoked: boolean, revocation_reason: string | null, };

export type PluginUploadResponse = { upload_id: string, expires_at: string, artifact: PluginArtifact, };

export type PluginInstallation = { installation_id: string, alias: string, package_id: string, last_operation: string, uninstalled: boolean, artifacts: Array<PluginArtifact>, };

export type PluginInstallationSummary = { installation_id: string, alias: string, package_id: string, last_operation: string, uninstalled: boolean, };

export type PluginListResponse = { installations: Array<PluginInstallationSummary>, next_cursor: string | null, };

export type PluginOperationKind = "install" | "add_artifact" | "revoke" | "uninstall";

export type PluginOperationResponse = { operation_id: string, kind: PluginOperationKind, installation: PluginInstallation, };

export type PluginArtifactDeliveryRequest = { plan_id: string, plan_fingerprint: string, installation_id: string, artifact_digest: string, };

export type PluginArtifactDeliveryMetadata = { workspace_id: string, request: PluginArtifactDeliveryRequest, artifact: PluginArtifact, };

export type PluginHostErrorCode = "invalid_input" | "denied" | "not_found" | "conflict" | "unsupported" | "quota_exceeded" | "deadline_exceeded" | "cancelled" | "unavailable" | "indeterminate" | "receipt_expired";

export type PluginWorkerDiagnostics = { workspace_id: string, runtime_id: string, worker_id: string, runtime_limits: PluginRuntimeLimits, selection: PluginWorkerDiagnosticSelection, };

export type PluginWorkerDiagnosticSelection = { "kind": "unrecorded" } | { "kind": "empty" } | { "kind": "plan", plan_id: string, fingerprint: string, pin_state: PluginDiagnosticPinState, plugins: Array<PluginWorkerDiagnostic>, };

export type PluginDiagnosticPinState = "pending" | "active" | "restorable" | "released";

export type PluginDiagnosticSurface = "tool" | "service";

export type PluginServiceConsumerDiagnostics = { service_name: string, diagnostics: PluginServiceDiagnostics, };

export type PluginWorkerDiagnostic = { installation_id: string, package_id: string, package_version: string, package_digest: string, artifact_digest: string, artifact_size: number, surfaces: Array<PluginDiagnosticSurface>, selected_tools: Array<string>, selected_services: Array<string>, service_consumers: Array<PluginServiceConsumerDiagnostics>, admission: PluginDiagnosticAdmission, connections: Array<PluginDiagnosticConnection>, };

export type PluginDiagnosticAdmission = { "kind": "admissible" } | { "kind": "refused", reason_code: PluginHostErrorCode, };

export type PluginDiagnosticConnection = { slot: string, effective_limits: PluginDiagnosticConnectionLimits, };

export type PluginDiagnosticConnectionLimits = { "kind": "available", timeout_ms: number, max_request_bytes: number, max_response_bytes: number, max_header_bytes: number, } | { "kind": "unavailable", reason_code: PluginHostErrorCode, };

export type PluginRuntimeLimits = { payload_bytes: number, config_bytes: number, memory_bytes: number, table_elements: number, instances: number, memories: number, tables: number, resources: number, fuel: number, wall_time_ms: number, host_operations: number, http_body_bytes: number, http_header_bytes: number, http_header_pairs: number, log_bytes: number, };
