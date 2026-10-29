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

export type PluginGrantMetadata = { "kind": "logging" } | { "kind": "https", slot: string, };

export type PluginArtifact = { package_id: string, name: string, version: string, description: string | null, package_digest: string, artifact_digest: string, artifact_size: number, schema_version: number, world: string, required_imports: Array<string>, tools: Array<PluginToolMetadata>, connections: Array<PluginConnectionMetadata>, requested_grants: Array<PluginGrantMetadata>, config_schema: PluginSchemaContribution, revoked: boolean, revocation_reason: string | null, };

export type PluginUploadResponse = { upload_id: string, expires_at: string, artifact: PluginArtifact, };

export type PluginInstallation = { installation_id: string, alias: string, package_id: string, last_operation: string, uninstalled: boolean, artifacts: Array<PluginArtifact>, };

export type PluginInstallationSummary = { installation_id: string, alias: string, package_id: string, last_operation: string, uninstalled: boolean, };

export type PluginListResponse = { installations: Array<PluginInstallationSummary>, next_cursor: string | null, };

export type PluginOperationKind = "install" | "add_artifact" | "revoke" | "uninstall";

export type PluginOperationResponse = { operation_id: string, kind: PluginOperationKind, installation: PluginInstallation, };

export type PluginArtifactDeliveryRequest = { plan_id: string, plan_fingerprint: string, installation_id: string, artifact_digest: string, };

export type PluginArtifactDeliveryMetadata = { workspace_id: string, request: PluginArtifactDeliveryRequest, artifact: PluginArtifact, };
