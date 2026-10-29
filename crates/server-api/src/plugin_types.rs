//! Generated plugin types share the exact DTOs used by the HTTP declarations.
#[cfg(feature = "typescript")]
pub fn plugin_host_typescript() -> String {
    use crate::*;
    use ts_rs::TS;
    let config = ts_rs::Config::default();
    let declarations = [
        PluginHttpMethod::decl(&config),
        PluginConsumer::decl(&config),
        PluginHostRequest::decl(&config),
        PluginHostOperation::decl(&config),
        PluginHttpRequest::decl(&config),
        PluginHttpResponse::decl(&config),
        PluginHostResponse::decl(&config),
        PluginLogEntry::decl(&config),
        PluginLogLevel::decl(&config),
        PluginLogField::decl(&config),
        PluginLogScalar::decl(&config),
        PluginHostError::decl(&config),
        PluginHostErrorCode::decl(&config),
        PluginEffectObservation::decl(&config),
    ];
    declarations.map(|d| format!("export {d}")).join("\n\n")
}

#[cfg(feature = "typescript")]
pub fn plugin_api_typescript() -> String {
    use crate::*;
    use ts_rs::TS;
    let config = ts_rs::Config::default();
    let declarations = [
        PluginInstallRequest::decl(&config),
        PluginAddArtifactRequest::decl(&config),
        PluginRevokeRequest::decl(&config),
        PluginUninstallRequest::decl(&config),
        PluginListRequest::decl(&config),
        PluginSchemaContribution::decl(&config),
        PluginToolMetadata::decl(&config),
        PluginHttpMethod::decl(&config),
        PluginConnectionMetadata::decl(&config),
        PluginGrantMetadata::decl(&config),
        PluginArtifact::decl(&config),
        PluginUploadResponse::decl(&config),
        PluginInstallation::decl(&config),
        PluginInstallationSummary::decl(&config),
        PluginListResponse::decl(&config),
        PluginOperationKind::decl(&config),
        PluginOperationResponse::decl(&config),
    ];
    format!(
        "// Generated from server-api. Do not edit by hand.\n// Regenerate: cargo run -q -p server-api --features typescript --example generate_plugin_api_types > web/workspace/src/lib/generated/plugin-api.ts\n\n{}\n",
        declarations.map(|d| format!("export {d}")).join("\n\n")
    )
}

#[cfg(test)]
mod tests {
    #[cfg(feature = "typescript")]
    #[test]
    fn host_typescript_preserves_wire_numbers_and_lossless_scalar_strings() {
        let text = super::plugin_host_typescript();
        for name in [
            "PluginHostRequest",
            "PluginHostResponse",
            "PluginHostError",
            "PluginConsumer",
            "PluginHttpRequest",
            "PluginHttpResponse",
            "PluginLogEntry",
        ] {
            assert!(text.contains(&format!("export type {name} =")), "{name}");
        }
        assert!(text.contains("expires_at_ms: number"));
        assert!(text.contains("\"signed\", \"value\": string"));
        assert!(text.contains("\"unsigned\", \"value\": string"));
    }
    #[test]
    fn plugin_routes_use_binary_upload_and_closed_management_contracts() {
        let document = crate::canonical_openapi_document().unwrap();
        let value: serde_json::Value = serde_json::from_str(&document.to_json().unwrap()).unwrap();
        let upload = &value["paths"]["/api/w/{workspace_id}/plugin-uploads"]["post"];
        assert!(upload["requestBody"]["content"]["application/octet-stream"].is_object());
        assert!(upload["requestBody"]["content"]["application/json"].is_null());
        for path in [
            "plugins",
            "plugins/{installation_id}",
            "plugin-operations/{operation_id}",
        ] {
            assert!(value["paths"][format!("/api/w/{{workspace_id}}/{path}")]["get"].is_object());
        }
        assert!(
            serde_json::from_value::<crate::PluginInstallRequest>(serde_json::json!({
                "upload_id":"x", "alias":"a", "operation_key":"k", "role":"owner"
            }))
            .is_err()
        );
        assert!(value["paths"]["/api/w/{workspace_id}/plugins/{installation_id}/enable"].is_null());
        assert!(value["paths"]["/api/w/{workspace_id}/plugins/{installation_id}/data"].is_null());
    }

    #[cfg(feature = "typescript")]
    #[test]
    fn generated_plugin_types_are_current() {
        assert_eq!(
            crate::normalized_typescript(super::plugin_api_typescript()),
            include_str!("../../../web/workspace/src/lib/generated/plugin-api.ts")
        );
    }
}
