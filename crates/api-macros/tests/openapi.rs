#![cfg(feature = "openapi")]
#![allow(async_fn_in_trait, dead_code)]

use api_macros::{
    BinaryBody, api,
    openapi::{OpenApiInfo, OpenApiSchema},
};
use schemars::JsonSchema;
use serde::{Deserialize, Deserializer, Serialize, Serializer};
use serde_json::{Value, json};

fn nullable_string_schema(generator: &mut schemars::SchemaGenerator) -> schemars::Schema {
    generator.subschema_for::<Option<String>>()
}

#[derive(Debug, Deserialize, JsonSchema, Serialize)]
pub struct GeoPoint {
    latitude: f64,
    longitude: f64,
}
impl OpenApiSchema for GeoPoint {}

/// A custom Serde wire representation admitted only after an explicit,
/// matching schema hook and `OpenApiSchema` attestation.
#[derive(Debug, JsonSchema)]
#[schemars(transparent)]
pub struct WireSlug(String);

impl Serialize for WireSlug {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        serializer.serialize_str(&self.0)
    }
}

impl<'de> Deserialize<'de> for WireSlug {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        String::deserialize(deserializer).map(Self)
    }
}
impl OpenApiSchema for WireSlug {}

#[derive(Debug, Deserialize, JsonSchema, Serialize)]
pub struct CreateWidget {
    #[schemars(length(min = 1, max = 64))]
    name: String,
    #[schemars(range(min = 1, max = 1_000))]
    count: u32,
    #[schemars(range(min = -100, max = 100))]
    bounded_balance: i64,
    #[schemars(length(min = 1, max = 8))]
    tags: Vec<String>,
    slug: WireSlug,
    /// Missing and explicit null are separate wire states.
    #[schemars(default, schema_with = "nullable_string_schema")]
    optional_label: Option<String>,
    #[schemars(required, schema_with = "nullable_string_schema")]
    required_nullable_label: Option<String>,
    location: GeoPoint,
}
impl OpenApiSchema for CreateWidget {}

#[derive(Debug, Deserialize, JsonSchema, Serialize)]
pub struct Lookup {
    detailed: bool,
}
impl OpenApiSchema for Lookup {}

#[derive(Debug, Deserialize, JsonSchema, Serialize)]
pub struct Widget {
    id: u32,
    request: CreateWidget,
}
impl OpenApiSchema for Widget {}

#[derive(Debug, Deserialize, JsonSchema, Serialize)]
#[serde(tag = "code", content = "details")]
pub enum PublicError {
    Invalid { message: String },
}
impl OpenApiSchema for PublicError {}

type OptionalRequestId = Option<String>;

#[api(openapi)]
pub trait FixtureApi {
    #[post(
        "/widgets/{widget_id}",
        operation_id = "widgets.create",
        status = 201,
        error_status = 422
    )]
    async fn create(
        &self,
        #[path] widget_id: u32,
        #[query] lookup: Lookup,
        #[header("authorization")] authorization: String,
        #[header("x-request-id")] request_id: OptionalRequestId,
        #[body] request: CreateWidget,
    ) -> Result<Widget, PublicError>;

    #[delete("/widgets/{widget_id}", operation_id = "widgets.delete", status = 204)]
    async fn delete(&self, #[path] widget_id: u32) -> ();

    #[put("/uploads", operation_id = "uploads.put", status = 200)]
    async fn upload(&self, #[binary] body: BinaryBody) -> Widget;

    #[get(
        "/conditional",
        operation_id = "conditional.get",
        responses = [
            (status = 200, body = Widget, headers = [("etag", String), ("cache-control", String)]),
            (status = 304, headers = [("etag", String), ("cache-control", String)])
        ]
    )]
    async fn conditional(&self) -> fixture_api_responses::Conditional;
}

fn document() -> api_macros::openapi::OpenApiDocument {
    fixture_api_openapi(OpenApiInfo {
        title: "Fixture API",
        version: "1.2.3",
        source_digest: "git:0123456789abcdef",
    })
    .expect("valid OpenAPI projection")
}

#[test]
fn output_is_deterministic_and_accepted_by_an_independent_parser() {
    let first = document().to_json().expect("render document");
    let second = document().to_json().expect("render document again");
    assert_eq!(first.as_bytes(), second.as_bytes());

    let parsed = oas3::from_json(&first).expect("oas3 independently parses generated document");
    assert_eq!(
        parsed
            .validate_version()
            .expect("valid OpenAPI version")
            .major,
        3
    );

    let value: Value = serde_json::from_str(&first).expect("valid JSON");
    assert_eq!(value["openapi"], "3.1.0");
    assert_eq!(value["info"]["x-yoi-source-digest"], "git:0123456789abcdef");
    assert!(
        value.get("servers").is_none(),
        "deployment topology is omitted"
    );
    assert!(!first.contains("generatedAt"));
    assert!(!first.contains("#/$defs/"));

    let schemas = value["components"]["schemas"]
        .as_object()
        .expect("component map");
    let mut references = Vec::new();
    collect_schema_references(&value, &mut references);
    assert!(!references.is_empty());
    for reference in references {
        let name = reference
            .strip_prefix("#/components/schemas/")
            .expect("only local component references are emitted");
        assert!(
            schemas.contains_key(name),
            "unresolved reference {reference}"
        );
    }
}

fn collect_schema_references<'a>(value: &'a Value, references: &mut Vec<&'a str>) {
    match value {
        Value::Object(object) => {
            if let Some(reference) = object.get("$ref").and_then(Value::as_str) {
                references.push(reference);
            }
            for value in object.values() {
                collect_schema_references(value, references);
            }
        }
        Value::Array(values) => {
            for value in values {
                collect_schema_references(value, references);
            }
        }
        _ => {}
    }
}

#[test]
fn operations_and_components_preserve_the_wire_contract() {
    let value = document().as_value().clone();
    let create = &value["paths"]["/widgets/{widget_id}"]["post"];
    assert_eq!(create["operationId"], "widgets.create");
    assert_eq!(create["security"], json!([{ "bearerAuth": [] }]));
    assert_eq!(
        create["responses"]["201"]["content"]["application/json"]["schema"],
        json!({
            "$ref": "#/components/schemas/Widget"
        })
    );
    assert_eq!(
        create["responses"]["422"]["content"]["application/json"]["schema"],
        json!({
            "$ref": "#/components/schemas/PublicError"
        })
    );
    assert_eq!(
        create["requestBody"]["content"]["application/json"]["schema"],
        json!({ "$ref": "#/components/schemas/CreateWidget" })
    );

    let parameters = create["parameters"].as_array().expect("parameter list");
    assert_eq!(parameters.len(), 3);
    assert_eq!(
        (parameters[0]["name"].as_str(), parameters[0]["in"].as_str()),
        (Some("widget_id"), Some("path"))
    );
    assert_eq!(parameters[0]["required"], true);
    assert_eq!(
        (parameters[1]["name"].as_str(), parameters[1]["in"].as_str()),
        (Some("lookup"), Some("query"))
    );
    assert_eq!(parameters[1]["explode"], true);
    assert_eq!(
        (parameters[2]["name"].as_str(), parameters[2]["in"].as_str()),
        (Some("x-request-id"), Some("header"))
    );
    assert_eq!(parameters[2]["required"], false);

    let schemas = value["components"]["schemas"]
        .as_object()
        .expect("components");
    assert_eq!(
        value["components"]["securitySchemes"]["bearerAuth"],
        json!({ "type": "http", "scheme": "bearer" })
    );
    for name in [
        "CreateWidget",
        "GeoPoint",
        "Lookup",
        "PublicError",
        "Widget",
        "WireSlug",
        "uint32",
        "Nullable_string",
    ] {
        assert!(
            schemas.contains_key(name),
            "missing stable schema component {name}; found {:?}",
            schemas.keys().collect::<Vec<_>>()
        );
    }
    assert_eq!(
        schemas["CreateWidget"]["properties"]["location"]["$ref"],
        "#/components/schemas/GeoPoint"
    );
    assert_eq!(
        schemas["CreateWidget"]["properties"]["slug"]["$ref"],
        "#/components/schemas/WireSlug"
    );
    assert_eq!(
        schemas["CreateWidget"]["properties"]["name"]["minLength"],
        1
    );
    assert_eq!(
        schemas["CreateWidget"]["properties"]["name"]["maxLength"],
        64
    );
    assert_eq!(
        schemas["CreateWidget"]["properties"]["count"]["maximum"],
        1_000
    );
    assert_eq!(
        schemas["CreateWidget"]["properties"]["bounded_balance"]["minimum"],
        -100
    );
    assert_eq!(
        schemas["CreateWidget"]["properties"]["bounded_balance"]["maximum"],
        100
    );
    assert_eq!(schemas["CreateWidget"]["properties"]["tags"]["minItems"], 1);
    assert_eq!(schemas["CreateWidget"]["properties"]["tags"]["maxItems"], 8);
    assert!(schemas["PublicError"]["oneOf"].is_array());

    let required = schemas["CreateWidget"]["required"]
        .as_array()
        .expect("required fields");
    assert!(!required.contains(&json!("optional_label")));
    assert!(required.contains(&json!("required_nullable_label")));
    for field in ["optional_label", "required_nullable_label"] {
        assert!(
            schemas["CreateWidget"]["properties"][field]["type"]
                .as_array()
                .expect("nullable type union")
                .contains(&json!("null"))
        );
    }

    let upload = &value["paths"]["/uploads"]["put"];
    assert_eq!(upload["operationId"], "uploads.put");
    assert_eq!(
        upload["requestBody"],
        json!({
            "required": true,
            "content": {
                "application/octet-stream": {
                    "schema": { "type": "string", "format": "binary" }
                }
            }
        })
    );
    assert!(
        !schemas.contains_key("BinaryBody"),
        "binary payloads are not JSON Schema components"
    );

    let conditional = &value["paths"]["/conditional"]["get"];
    assert_eq!(
        conditional["responses"]["200"]["content"]["application/json"]["schema"],
        json!({ "$ref": "#/components/schemas/Widget" })
    );
    assert_eq!(
        conditional["responses"]["200"]["headers"]["etag"]["schema"],
        json!({ "$ref": "#/components/schemas/string" })
    );
    assert_eq!(
        conditional["responses"]["304"],
        json!({
            "description": "Alternate successful response",
            "headers": {
                "cache-control": { "schema": { "$ref": "#/components/schemas/string" } },
                "etag": { "schema": { "$ref": "#/components/schemas/string" } }
            }
        })
    );

    let delete = &value["paths"]["/widgets/{widget_id}"]["delete"];
    assert_eq!(delete["operationId"], "widgets.delete");
    assert!(delete.get("security").is_none());
    assert_eq!(
        delete["responses"]["204"],
        json!({ "description": "Successful response" })
    );
}

#[derive(Debug, JsonSchema)]
pub struct FirstCollision {
    first: String,
}
impl OpenApiSchema for FirstCollision {
    fn openapi_schema_name() -> std::borrow::Cow<'static, str> {
        "SharedWireName".into()
    }
}

#[derive(Debug, JsonSchema)]
pub struct SecondCollision {
    second: bool,
}
impl OpenApiSchema for SecondCollision {
    fn openapi_schema_name() -> std::borrow::Cow<'static, str> {
        "SharedWireName".into()
    }
}

#[derive(Debug, JsonSchema)]
#[serde(untagged)]
pub enum AmbiguousWireEnum {
    Text(String),
    Flag(bool),
}
impl OpenApiSchema for AmbiguousWireEnum {}

#[derive(Debug)]
pub struct DanglingReference;

impl JsonSchema for DanglingReference {
    fn schema_name() -> std::borrow::Cow<'static, str> {
        "DanglingReference".into()
    }

    fn json_schema(_generator: &mut schemars::SchemaGenerator) -> schemars::Schema {
        schemars::json_schema!({ "$ref": "#/$defs/Missing" })
    }
}
impl OpenApiSchema for DanglingReference {}

#[test]
fn schema_name_collisions_and_ambiguous_wire_shapes_fail_closed() {
    use api_macros::openapi::{OpenApiBuilder, OpenApiError};

    let info = OpenApiInfo {
        title: "Rejected fixture",
        version: "1",
        source_digest: "git:bad",
    };
    let mut collision = OpenApiBuilder::new(info).expect("builder");
    {
        let mut operation = collision
            .operation("GET", "/first", "first")
            .expect("operation");
        operation
            .response::<FirstCollision>(200, "application/json", "ok")
            .expect("first schema");
        operation.finish().expect("first operation");
    }
    let error = collision
        .operation("GET", "/second", "second")
        .expect("operation")
        .response::<SecondCollision>(200, "application/json", "ok")
        .expect_err("different schemas must not share a public name");
    assert!(matches!(error, OpenApiError::InvalidContract(_)));
    assert!(error.to_string().contains("component name collision"));

    let mut ambiguous = OpenApiBuilder::new(info).expect("builder");
    let error = ambiguous
        .operation("GET", "/ambiguous", "ambiguous")
        .expect("operation")
        .response::<AmbiguousWireEnum>(200, "application/json", "ok")
        .expect_err("untagged/ambiguous unions must fail closed");
    assert!(error.to_string().contains("untagged"));

    let mut dangling = OpenApiBuilder::new(info).expect("builder");
    {
        let mut operation = dangling
            .operation("GET", "/dangling", "dangling")
            .expect("operation");
        operation
            .response::<DanglingReference>(200, "application/json", "ok")
            .expect("schema registration succeeds before graph validation");
        operation.finish().expect("operation");
    }
    let error = dangling
        .finish()
        .expect_err("dangling component references must fail closed");
    assert!(error.to_string().contains("unresolved"));
}

#[test]
fn file_export_writes_the_canonical_document() {
    let path = std::env::temp_dir().join(format!(
        "yoi-openapi-{}-{}.json",
        std::process::id(),
        std::thread::current().name().unwrap_or("test")
    ));
    let document = document();
    document.write_json(&path).expect("export OpenAPI file");
    let written = std::fs::read_to_string(&path).expect("read export");
    std::fs::remove_file(path).expect("remove export");
    assert_eq!(written, document.to_json().expect("canonical JSON"));
}
