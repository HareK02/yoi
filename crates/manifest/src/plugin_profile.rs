//! Narrowing requests for Workspace-managed Plugin installations.

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};

/// Profile input only, not Workspace enablement or a validated execution plan.
///
/// The Workspace owner must resolve each installation against its enabled exact
/// artifact and reject selections outside its Tool/HTTPS/logging ceiling. Neither
/// these names nor config values grant authority, select package paths, or cause
/// discovery, dynamic registration, or guest execution.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PluginProfileConfig {
    #[serde(default)]
    pub selections: Vec<PluginProfileSelection>,
}

impl PluginProfileConfig {
    pub fn is_empty(&self) -> bool {
        self.selections.is_empty()
    }
}

/// One installation's explicit subset; omitted surfaces and capabilities are off.
///
/// `tools` and `https_slots` are package-declared names, not public Tool names,
/// URLs, host paths, or connection bindings. `config` contains non-secret override
/// candidates only: the Workspace schema validator must reject them unless the
/// package's closed typed schema permits those overrides. Deserialization alone
/// cannot certify that arbitrary package-specific values are non-secret or valid.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PluginProfileSelection {
    pub installation_id: String,
    #[serde(default)]
    pub tools: Vec<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub services: Vec<String>,
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub ticket_events: bool,
    #[serde(default)]
    pub logging: bool,
    #[serde(default)]
    pub https_slots: Vec<String>,
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub ticket_read: bool,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub ticket_data_read: Vec<PluginTicketDataGrant>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub ticket_data_write: Vec<PluginTicketDataGrant>,
    #[serde(default)]
    pub config: BTreeMap<String, serde_json::Value>,
}

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PluginTicketDataGrant {
    pub collection: String,
    pub schema_id: String,
    pub schema_version: String,
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        ProfileSource, WorkerManifest, WorkerManifestConfig,
        read_persisted_worker_manifest_snapshot, resolve_profile_artifact_value,
        write_persisted_worker_manifest_snapshot,
    };
    use serde_json::{Value, json};
    use sha2::{Digest, Sha256};

    fn partial(plugins: Value) -> WorkerManifestConfig {
        serde_json::from_value(json!({
            "worker": { "name": "plugin-worker" },
            "plugins": plugins,
        }))
        .unwrap()
    }

    fn resolve_profile(plugins: Value) -> crate::ResolvedProfile {
        let dir = tempfile::tempdir().unwrap();
        resolve_profile_artifact_value(
            json!({ "plugins": plugins }),
            ProfileSource::Archive {
                archive_id: "profile-input".into(),
                source: "test".into(),
            },
            dir.path(),
            "plugin-worker",
        )
        .unwrap()
    }

    #[test]
    fn omitted_plugin_selections_and_grants_default_off() {
        let omitted = WorkerManifestConfig::from_toml("[worker]\nname = 'plain'\n").unwrap();
        assert!(
            serde_json::to_value(&omitted)
                .unwrap()
                .get("plugins")
                .is_none()
        );
        let manifest = WorkerManifest::try_from(omitted).unwrap();
        assert!(manifest.plugins.selections.is_empty());
        assert!(!manifest.requires_persisted_execution_snapshot());
        assert!(
            serde_json::to_value(manifest)
                .unwrap()
                .get("plugins")
                .is_none()
        );

        let manifest = WorkerManifest::try_from(partial(json!({
            "selections": [{ "installation_id": "installation-a" }]
        })))
        .unwrap();
        let selection = &manifest.plugins.selections[0];
        assert!(selection.tools.is_empty());
        assert!(!selection.logging);
        assert!(selection.https_slots.is_empty());
        assert!(selection.config.is_empty());
        assert!(manifest.requires_persisted_execution_snapshot());
    }

    #[test]
    fn plugin_partial_merge_replaces_instead_of_unioning_and_can_clear() {
        let lower = partial(json!({ "selections": [{
            "installation_id": "installation-a", "tools": ["read", "write"],
            "logging": true, "https_slots": ["remote"], "config": {"limit": 10}
        }] }));
        let omitted = lower.clone().merge(WorkerManifestConfig::default());
        assert_eq!(omitted.plugins, lower.plugins);

        let upper = partial(json!({ "selections": [{
            "installation_id": "installation-a", "tools": ["read"]
        }] }));
        let merged = lower.clone().merge(upper.clone());
        assert_eq!(merged.plugins, upper.plugins);
        let round_trip: WorkerManifestConfig =
            serde_json::from_value(serde_json::to_value(&merged).unwrap()).unwrap();
        assert_eq!(round_trip.plugins, merged.plugins);

        for empty in [json!({}), json!({ "selections": [] })] {
            let cleared = lower.clone().merge(partial(empty));
            assert_eq!(cleared.plugins, Some(PluginProfileConfig::default()));
            let manifest = WorkerManifest::try_from(cleared).unwrap();
            assert!(manifest.plugins.selections.is_empty());
        }
    }

    #[test]
    fn plugin_profile_toml_and_resolved_snapshots_preserve_typed_overrides() {
        let dir = tempfile::tempdir().unwrap();
        let source = r#"
[[plugins.selections]]
installation_id = "installation-a"
tools = ["lookup"]
logging = true
https_slots = ["api"]
[plugins.selections.config]
limit = 5
labels = ["one", "two"]
[plugins.selections.config.nested]
strict = true
"#;
        let parsed = WorkerManifestConfig::from_toml(source).unwrap();
        let authored: toml::Value = toml::from_str(source).unwrap();
        let resolved = resolve_profile_artifact_value(
            serde_json::to_value(authored).unwrap(),
            ProfileSource::Archive {
                archive_id: "typed-profile".into(),
                source: "test".into(),
            },
            dir.path(),
            "plugin-worker",
        )
        .unwrap();
        assert_eq!(Some(&resolved.manifest.plugins), parsed.plugins.as_ref());
        assert_eq!(
            resolved.manifest.plugins.selections[0].config["nested"],
            json!({"strict": true})
        );
        let snapshot = write_persisted_worker_manifest_snapshot(&resolved.manifest).unwrap();
        let restored = read_persisted_worker_manifest_snapshot(snapshot.clone()).unwrap();
        assert_eq!(restored.plugins, resolved.manifest.plugins);
        assert_eq!(
            write_persisted_worker_manifest_snapshot(&restored).unwrap(),
            snapshot
        );
        assert_eq!(
            serde_json::to_value(restored).unwrap(),
            resolved.manifest_snapshot
        );
        let toml_round_trip = toml::to_string(&resolved.manifest).unwrap();
        assert_eq!(
            WorkerManifest::from_toml(&toml_round_trip).unwrap().plugins,
            resolved.manifest.plugins
        );
    }

    #[test]
    fn plugin_fields_change_serialized_profile_fingerprint_input() {
        let baseline = json!({ "selections": [{
            "installation_id": "installation-a", "tools": ["read"],
            "logging": false, "https_slots": [], "config": {"limit": 5}
        }] });
        let fingerprint = |plugins| {
            let resolved = resolve_profile(plugins);
            Sha256::digest(serde_json::to_vec(&resolved.manifest_snapshot).unwrap())
        };
        let original = fingerprint(baseline.clone());
        assert_eq!(fingerprint(baseline.clone()), original);
        for (field, value) in [
            ("installation_id", json!("installation-b")),
            ("tools", json!(["write"])),
            ("logging", json!(true)),
            ("https_slots", json!(["api"])),
            ("config", json!({"limit": 6})),
        ] {
            let mut changed = baseline.clone();
            changed["selections"][0][field] = value;
            assert_ne!(fingerprint(changed), original, "{field} was lost");
        }
        assert_eq!(
            fingerprint(json!({})),
            fingerprint(json!({"selections": []}))
        );
    }

    #[test]
    fn plugin_config_rejects_legacy_package_and_unknown_authority_fields() {
        let dir = tempfile::tempdir().unwrap();
        let mut invalid = vec![
            json!({ "enabled": [{ "id": "project:legacy" }] }),
            json!({ "packages": ["/tmp/legacy.yoi-plugin"] }),
            json!({ "resolved": [{ "package_path": "/tmp/legacy.yoi-plugin" }] }),
            json!({ "enable": true }),
        ];
        for (field, value) in [
            ("package_path", json!("/tmp/legacy.yoi-plugin")),
            ("path", json!("./plugin")),
            ("url", json!("https://example.invalid/plugin")),
            ("artifact_digest", json!("sha256:uninstalled")),
            ("secret", json!("not-a-secret-reference")),
            ("data", json!(true)),
            ("events", json!(["changed"])),
            ("websocket", json!(true)),
            ("workdir", json!(true)),
            ("unknown", json!(true)),
        ] {
            let mut selection = json!({ "installation_id": "installation-a" });
            selection[field] = value;
            invalid.push(json!({ "selections": [selection] }));
        }
        invalid.extend([
            json!({ "selections": [{ "tools": ["read"] }] }),
            json!({ "selections": [{ "installation_id": "a", "config": "raw" }] }),
        ]);
        for plugins in invalid {
            let partial_input = json!({ "plugins": plugins.clone() });
            assert!(
                serde_json::from_value::<WorkerManifestConfig>(partial_input.clone()).is_err(),
                "{plugins}"
            );
            let toml = toml::to_string(&partial_input).unwrap();
            assert!(WorkerManifestConfig::from_toml(&toml).is_err(), "{toml}");
            let error = resolve_profile_artifact_value(
                partial_input,
                ProfileSource::Archive {
                    archive_id: "invalid".into(),
                    source: "test".into(),
                },
                dir.path(),
                "plugin-worker",
            )
            .unwrap_err();
            assert!(
                error.to_string().contains("failed to decode Profile"),
                "{error}"
            );
        }
    }
}

#[cfg(test)]
mod service_tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn service_selection_is_explicit_and_old_profiles_remain_off() {
        let old: PluginProfileSelection =
            serde_json::from_value(json!({"installation_id":"i", "tools":["read"]})).unwrap();
        assert!(old.services.is_empty());
        assert!(!old.ticket_events);
        let retained = serde_json::to_value(&old).unwrap();
        assert!(retained.get("services").is_none());
        assert!(retained.get("ticket_events").is_none());
        let selected: PluginProfileSelection = serde_json::from_value(
            json!({"installation_id":"i", "services":["watch"], "ticket_events":true}),
        )
        .unwrap();
        assert_eq!(selected.services, ["watch"]);
        assert!(selected.ticket_events);
        assert_ne!(serde_json::to_value(selected).unwrap(), retained);
    }
}
