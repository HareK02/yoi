use super::*;
use serde_json::{Value, json};

fn input() -> Value {
    json!({
        "origin": "https://api.example.com",
        "methods": ["GET", "POST"],
        "path_prefixes": ["/api", "/v2"],
        "request_headers": ["accept", "content-type"],
        "response_headers": ["content-type", "etag"],
        "secret_ref": "secret-123",
        "limits": {
            "timeout_ms": 1000,
            "max_request_bytes": 1024,
            "max_response_bytes": 4096,
            "max_header_bytes": 512
        }
    })
}

fn binding(value: Value) -> PluginConnectionBinding {
    serde_json::from_value(value).unwrap()
}

#[test]
fn origin_input_table_requires_canonical_exact_https_and_public_destination() {
    for (origin, expected) in [
        ("https://api.example.com", Ok(())),
        ("https://api.example.com:8443", Ok(())),
        ("https://8.8.8.8", Ok(())),
        ("https://[2606:4700:4700::1111]", Ok(())),
        (
            "http://api.example.com",
            Err(PluginBindingError::InvalidOrigin),
        ),
        (
            "https://api.example.com/",
            Err(PluginBindingError::InvalidOrigin),
        ),
        (
            "https://api.example.com/api",
            Err(PluginBindingError::InvalidOrigin),
        ),
        (
            "https://api.example.com?token=value",
            Err(PluginBindingError::InvalidOrigin),
        ),
        (
            "https://api.example.com#fragment",
            Err(PluginBindingError::InvalidOrigin),
        ),
        (
            "https://user:password@api.example.com",
            Err(PluginBindingError::InvalidOrigin),
        ),
        (
            "https://@api.example.com",
            Err(PluginBindingError::InvalidOrigin),
        ),
        (
            "https://api.example.com:443",
            Err(PluginBindingError::InvalidOrigin),
        ),
        (
            "https://API.example.com",
            Err(PluginBindingError::InvalidOrigin),
        ),
        (
            "https://api.example.com\\evil",
            Err(PluginBindingError::InvalidOrigin),
        ),
        (
            "https://api.example.com\n",
            Err(PluginBindingError::InvalidOrigin),
        ),
        (
            "https://*.example.com",
            Err(PluginBindingError::NonPublicDestination),
        ),
        (
            "https://localhost",
            Err(PluginBindingError::NonPublicDestination),
        ),
        (
            "https://metadata.google.internal",
            Err(PluginBindingError::NonPublicDestination),
        ),
        (
            "https://service.local",
            Err(PluginBindingError::NonPublicDestination),
        ),
        (
            "https://example.com.",
            Err(PluginBindingError::NonPublicDestination),
        ),
        (
            "https://127.0.0.1",
            Err(PluginBindingError::NonPublicDestination),
        ),
        (
            "https://10.0.0.1",
            Err(PluginBindingError::NonPublicDestination),
        ),
        (
            "https://172.16.0.1",
            Err(PluginBindingError::NonPublicDestination),
        ),
        (
            "https://192.168.1.1",
            Err(PluginBindingError::NonPublicDestination),
        ),
        (
            "https://169.254.169.254",
            Err(PluginBindingError::NonPublicDestination),
        ),
        (
            "https://100.64.0.1",
            Err(PluginBindingError::NonPublicDestination),
        ),
        (
            "https://192.0.2.1",
            Err(PluginBindingError::NonPublicDestination),
        ),
        (
            "https://198.51.100.1",
            Err(PluginBindingError::NonPublicDestination),
        ),
        (
            "https://203.0.113.1",
            Err(PluginBindingError::NonPublicDestination),
        ),
        (
            "https://224.1.2.3",
            Err(PluginBindingError::NonPublicDestination),
        ),
        (
            "https://0.0.0.0",
            Err(PluginBindingError::NonPublicDestination),
        ),
        ("https://2130706433", Err(PluginBindingError::InvalidOrigin)),
        ("https://0177.0.0.1", Err(PluginBindingError::InvalidOrigin)),
        (
            "https://[::1]",
            Err(PluginBindingError::NonPublicDestination),
        ),
        (
            "https://[::ffff:7f00:1]",
            Err(PluginBindingError::NonPublicDestination),
        ),
        (
            "https://[fd00::1]",
            Err(PluginBindingError::NonPublicDestination),
        ),
        (
            "https://[fe80::1]",
            Err(PluginBindingError::NonPublicDestination),
        ),
        (
            "https://[2001:db8::1]",
            Err(PluginBindingError::NonPublicDestination),
        ),
        (
            "https://[2002:7f00:1::]",
            Err(PluginBindingError::NonPublicDestination),
        ),
    ] {
        let mut value = input();
        value["origin"] = json!(origin);
        assert_eq!(
            binding(value).validate(&PluginConnectionPolicyCeiling::default()),
            expected,
            "{origin}"
        );
    }
}

#[test]
fn path_input_table_rejects_traversal_and_preserves_segment_boundaries() {
    for (path, accepted) in [
        ("/", true),
        ("/api", true),
        ("/api/v1", true),
        ("api", false),
        ("//evil.example", false),
        ("/api/", false),
        ("/api//v1", false),
        ("/api/../admin", false),
        ("/./api", false),
        ("/api\\admin", false),
        ("/api?x=1", false),
        ("/api#x", false),
        ("/api/%2e%2e/admin", false),
        ("/api%2fadmin", false),
        ("/api/%252e%252e", false),
        ("/api/%5Cadmin", false),
        ("https://evil.example/api", false),
        ("/api/\n", false),
    ] {
        let mut value = input();
        value["path_prefixes"] = json!([path]);
        assert_eq!(
            binding(value)
                .validate(&PluginConnectionPolicyCeiling::default())
                .is_ok(),
            accepted,
            "{path}"
        );
    }
    let value = binding(input());
    for (path, allowed) in [
        ("/api", true),
        ("/api/v1", true),
        ("/apix", false),
        ("/admin", false),
        ("/api/../admin", false),
    ] {
        assert_eq!(value.allows_path(path), allowed, "{path}");
    }
    for paths in [json!([]), json!(["/api", "/api"])] {
        let mut value = input();
        value["path_prefixes"] = paths;
        assert_eq!(
            binding(value).validate(&PluginConnectionPolicyCeiling::default()),
            Err(PluginBindingError::InvalidPaths)
        );
    }
}

#[test]
fn header_input_table_rejects_guest_credentials_and_transport_controls() {
    for field in ["request_headers", "response_headers"] {
        for (name, accepted) in [
            ("accept", true),
            ("content-type", true),
            ("x-request-id", true),
            ("authorization", false),
            ("cookie", false),
            ("set-cookie", false),
            ("host", false),
            ("proxy-authorization", false),
            ("connection", false),
            ("content-length", false),
            ("transfer-encoding", false),
            ("upgrade", false),
            ("te", false),
            ("trailer", false),
            ("keep-alive", false),
            ("sec-websocket-key", false),
            ("x-api-key", false),
            ("Content-Type", false),
            ("x-a\r\n", false),
            ("x-a: value", false),
            ("", false),
        ] {
            let mut value = input();
            value[field] = json!([name]);
            assert_eq!(
                binding(value)
                    .validate(&PluginConnectionPolicyCeiling::default())
                    .is_ok(),
                accepted,
                "{field}:{name}"
            );
        }
        let mut value = input();
        value[field] = json!(["accept", "accept"]);
        assert_eq!(
            binding(value).validate(&PluginConnectionPolicyCeiling::default()),
            Err(PluginBindingError::InvalidHeaders)
        );
    }
}

#[test]
fn limits_input_table_requires_positive_finite_values_within_each_server_bound() {
    let ceiling = PluginConnectionPolicyCeiling::default();
    for (field, max) in [
        ("timeout_ms", ceiling.limits.timeout_ms),
        ("max_request_bytes", ceiling.limits.max_request_bytes),
        ("max_response_bytes", ceiling.limits.max_response_bytes),
        ("max_header_bytes", ceiling.limits.max_header_bytes),
    ] {
        for (amount, accepted) in [(0, false), (1, true), (max, true), (max + 1, false)] {
            let mut value = input();
            value["limits"][field] = json!(amount);
            assert_eq!(
                binding(value).validate(&ceiling).is_ok(),
                accepted,
                "{field}:{amount}"
            );
        }
        for invalid in [
            json!(-1),
            json!(1.5),
            json!(4294967296u64),
            json!("unlimited"),
            Value::Null,
        ] {
            let mut value = input();
            value["limits"][field] = invalid;
            assert!(
                serde_json::from_value::<PluginConnectionBinding>(value).is_err(),
                "{field}"
            );
        }
    }
    let mut zero_ceiling = ceiling;
    zero_ceiling.limits.timeout_ms = 0;
    assert_eq!(
        binding(input()).validate(&zero_ceiling),
        Err(PluginBindingError::InvalidLimits)
    );
}

#[test]
fn secret_and_identity_inputs_accept_only_opaque_workspace_references() {
    for (reference, accepted) in [
        ("", true),
        ("secret_123-a", true),
        ("/etc/secret", false),
        ("file:///etc/secret", false),
        ("https://secret.example/token", false),
        ("Bearer token", false),
        ("env:API_KEY", false),
        ("../secret", false),
    ] {
        let mut value = input();
        value["secret_ref"] = json!(reference);
        assert_eq!(
            binding(value)
                .validate(&PluginConnectionPolicyCeiling::default())
                .is_ok(),
            accepted,
            "{reference}"
        );
    }
    for (identity, accepted) in [
        ("api-binding", true),
        ("", false),
        ("../binding", false),
        ("https://example.com", false),
    ] {
        let bindings = BTreeMap::from([(identity.to_string(), binding(input()))]);
        assert_eq!(
            validate_bindings(&bindings, &PluginConnectionPolicyCeiling::default()).is_ok(),
            accepted
        );
    }
}

#[test]
fn json_boundary_is_closed_and_methods_network_are_not_ambient_authority() {
    for (field, value) in [
        ("secret_value", json!("password")),
        ("token", json!("token")),
        ("proxy", json!("https://proxy.example")),
        ("host_path", json!("/tmp")),
        ("tls_verify", json!(false)),
        ("allow_private", json!(true)),
        ("network", json!("private")),
        ("methods", json!(["CONNECT"])),
    ] {
        let mut source = input();
        source[field] = value;
        assert!(
            serde_json::from_value::<PluginConnectionBinding>(source).is_err(),
            "{field}"
        );
    }
    for methods in [json!([]), json!(["GET", "GET"])] {
        let mut source = input();
        source["methods"] = methods;
        assert_eq!(
            binding(source).validate(&PluginConnectionPolicyCeiling::default()),
            Err(PluginBindingError::InvalidMethods)
        );
    }
    let mut source = input();
    source["limits"]["unlimited"] = json!(true);
    assert!(serde_json::from_value::<PluginConnectionBinding>(source).is_err());
}

#[test]
fn policy_digest_is_order_independent_and_covers_every_policy_field() {
    let original = binding(input());
    let mut reordered = original.clone();
    reordered.methods.reverse();
    reordered.path_prefixes.reverse();
    reordered.request_headers.reverse();
    reordered.response_headers.reverse();
    assert_eq!(original.digest(), reordered.digest());
    assert_eq!(
        original.digest(),
        "sha256:df6bd126eb6e825349a3c2e40f48f9d18732969a0775fe6f1b7f11b1667869c9"
    );
    assert_eq!(original.digest().len(), 71);
    assert!(original.digest().starts_with("sha256:"));
    for (field, value) in [
        ("origin", json!("https://other.example.com")),
        ("methods", json!(["GET"])),
        ("path_prefixes", json!(["/v2"])),
        ("request_headers", json!([])),
        ("response_headers", json!([])),
        ("secret_ref", json!("other-secret")),
    ] {
        let mut source = input();
        source[field] = value;
        assert_ne!(original.digest(), binding(source).digest(), "{field}");
    }
    for field in [
        "timeout_ms",
        "max_request_bytes",
        "max_response_bytes",
        "max_header_bytes",
    ] {
        let mut source = input();
        source["limits"][field] = json!(2);
        assert_ne!(original.digest(), binding(source).digest(), "{field}");
    }
}

#[test]
fn policy_collections_and_references_have_finite_size_bounds() {
    let ceiling = PluginConnectionPolicyCeiling::default();
    for (count, accepted) in [(64, true), (65, false)] {
        for field in ["path_prefixes", "request_headers", "response_headers"] {
            let mut source = input();
            source[field] = json!(
                (0..count)
                    .map(|n| {
                        if field == "path_prefixes" {
                            format!("/path-{n}")
                        } else {
                            format!("x-header-{n}")
                        }
                    })
                    .collect::<Vec<_>>()
            );
            assert_eq!(
                binding(source).validate(&ceiling).is_ok(),
                accepted,
                "{field}:{count}"
            );
        }
    }
    for (count, accepted) in [(256, true), (257, false)] {
        let bindings = (0..count)
            .map(|n| (format!("binding-{n}"), binding(input())))
            .collect();
        assert_eq!(validate_bindings(&bindings, &ceiling).is_ok(), accepted);
    }
    for (size, accepted) in [(128, true), (129, false)] {
        let mut source = input();
        source["secret_ref"] = json!("a".repeat(size));
        assert_eq!(binding(source).validate(&ceiling).is_ok(), accepted);
        let bindings = BTreeMap::from([("a".repeat(size), binding(input()))]);
        assert_eq!(validate_bindings(&bindings, &ceiling).is_ok(), accepted);
    }
}

fn evaluate_schema(source: &str) -> Result<Value, String> {
    use config_source::{
        ConfigContentType, ConfigEntry, ConfigSchemaContribution, ConfigTreeSnapshot,
        SnapshotEnvironment, ToolchainContract, VirtualPath, WorkspaceConfigSchemaBundle,
    };
    let path = VirtualPath::parse("main.dcdl").unwrap();
    let snapshot = ConfigTreeSnapshot::from_entries([ConfigEntry::new(
        path.clone(),
        ConfigContentType::Decodal,
        source,
    )
    .unwrap()])
    .unwrap();
    let bundle = WorkspaceConfigSchemaBundle::compose([ConfigSchemaContribution::new(
        "builtin:plugins",
        "plugins",
        "1",
        include_str!("../../../../resources/config-schema/plugins.dcdl"),
    )
    .unwrap()])
    .unwrap();
    let result = SnapshotEnvironment::new(snapshot).evaluate_contract(
        &ToolchainContract::with_schema_bundle(1, vec![path], 1, bundle),
    );
    result
        .map(|evaluated| evaluated.projections[0].data_json.clone())
        .map_err(|error| format!("{error:?}"))
}

#[test]
fn bootstrap_schema_defaults_maps_and_projects_typed_binding_to_rust() {
    assert_eq!(
        evaluate_schema("{}").unwrap(),
        json!({"plugins": {"enabled": {}, "bindings": {}}})
    );
    let projected = evaluate_schema(r#"{
        plugins = {
            enabled = { demo = {
                installation_id = "plugin-1"; expected_last_operation = "op-1";
                package_digest = "package"; artifact_digest = "artifact"; tools = ["query"];
            }; };
            bindings = { api = {
                origin = "https://api.example.com"; methods = ["GET"]; path_prefixes = ["/api"];
                limits = { timeout_ms = 1000; max_request_bytes = 1024; max_response_bytes = 4096; max_header_bytes = 512; };
            }; };
        };
    }"#).unwrap();
    assert_eq!(projected["plugins"]["enabled"]["demo"]["logging"], false);
    assert_eq!(
        projected["plugins"]["enabled"]["demo"]["connections"],
        json!({})
    );
    assert_eq!(projected["plugins"]["enabled"]["demo"]["config"], json!({}));
    let bindings: BTreeMap<String, PluginConnectionBinding> =
        serde_json::from_value(projected["plugins"]["bindings"].clone()).unwrap();
    validate_bindings(&bindings, &PluginConnectionPolicyCeiling::default()).unwrap();
}

#[test]
fn bootstrap_schema_rejects_unknown_fields_and_wrong_binding_shapes() {
    for source in [
        "{ plugins = { token = \"value\"; }; }",
        "{ plugins = { bindings = { api = { origin = 123; }; }; }; }",
        "{ plugins = { enabled = { demo = { arbitrary = true; }; }; }; }",
        "{ plugins = { bindings = { api = \"https://api.example.com\"; }; }; }",
    ] {
        assert!(evaluate_schema(source).is_err(), "{source}");
    }
}
