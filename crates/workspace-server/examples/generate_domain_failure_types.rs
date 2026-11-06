//! Generate domain-owned details and identity checks for the shared Web receiver.
//! This is a build artifact generator, not a second wire codec or HTTP boundary.
use std::{collections::BTreeSet, error::Error, fs, path::Path};

use error_contract::FailureCause;
use schemars::JsonSchema;
use serde::de::DeserializeOwned;
use serde_json::{Map, Value, json};
use server_api::repository_openapi_typescript::generate_schema_closure_typescript;
use workdir::failure::WorkdirFailureDetails;
use worker::feature::builtin::{
    drive::failure::DriveClientFailureDetails, workspace_config::failure::ConfigFailureDetails,
};
use workspace_drive::failure::DriveFailureDetails;
use yoi_workspace_server::{
    configuration_failure::ConfigurationFailureDetails,
    repository_access_failure::RepositoryAccessDetails,
    repository_failure::RepositoryFailureDetails,
};

const OUTPUT: &str = "web/workspace/src/lib/generated/domain-failures.ts";

fn rewrite_refs(value: &mut Value) {
    match value {
        Value::Object(object) => {
            if let Some(Value::String(reference)) = object.get_mut("$ref") {
                if let Some(name) = reference.strip_prefix("#/$defs/") {
                    *reference = format!("#/components/schemas/{name}");
                }
            }
            for child in object.values_mut() {
                rewrite_refs(child);
            }
        }
        Value::Array(values) => values.iter_mut().for_each(rewrite_refs),
        _ => {}
    }
}

// Disclose only closed enums and closed objects composed from them. Refuse free
// text or maps rather than silently widening the domain allowlist in Web.
fn field_contract(schema: &Value, schemas: &Map<String, Value>) -> Result<Value, Box<dyn Error>> {
    if let Some(reference) = schema["$ref"].as_str() {
        let name = reference
            .strip_prefix("#/components/schemas/")
            .ok_or("unexpected schema reference")?;
        return field_contract(schemas.get(name).ok_or("missing enum schema")?, schemas);
    }
    if let Some(values) = schema["enum"].as_array() {
        if values.iter().all(Value::is_string) {
            return Ok(json!(values));
        }
    }
    if schema["type"] == "null" {
        return Ok(json!([null]));
    }
    if let Some(value) = schema["const"].as_str() {
        return Ok(json!([value]));
    }
    if let Some(branches) = schema["anyOf"]
        .as_array()
        .or_else(|| schema["oneOf"].as_array())
    {
        let contracts: Vec<Value> = branches
            .iter()
            .map(|branch| field_contract(branch, schemas))
            .collect::<Result<_, _>>()?;
        return Ok(if contracts.iter().all(Value::is_array) {
            Value::Array(
                contracts
                    .into_iter()
                    .flat_map(|contract| contract.as_array().unwrap().clone())
                    .collect(),
            )
        } else {
            json!({"any_of": contracts})
        });
    }
    if schema["type"] == "object" && schema["additionalProperties"] == false {
        let fields = schema["properties"]
            .as_object()
            .ok_or("closed object properties required")?
            .iter()
            .map(|(key, value)| Ok((key.clone(), field_contract(value, schemas)?)))
            .collect::<Result<Map<String, Value>, Box<dyn Error>>>()?;
        return Ok(
            json!({"fields":fields, "required":schema["required"].as_array().cloned().unwrap_or_default()}),
        );
    }
    Err(format!("domain details are no longer a closed-enum allowlist: {schema}").into())
}

#[derive(Default)]
struct Artifacts {
    schemas: Map<String, Value>,
    roots: BTreeSet<String>,
    contracts: Map<String, Value>,
}
impl Artifacts {
    fn register<T: JsonSchema + DeserializeOwned + FailureCause>(
        &mut self,
        mut seed: Value,
        category_field: &str,
        reason_fields: &[&str],
    ) -> Result<(), Box<dyn Error>> {
        let mut schema = serde_json::to_value(schemars::schema_for!(T))?;
        rewrite_refs(&mut schema);
        let root = schema.as_object_mut().ok_or("object schema required")?;
        root.remove("$schema");
        if let Some(Value::Object(definitions)) = root.remove("$defs") {
            for (name, mut value) in definitions {
                // Root and referenced Schemars definitions differ only in title metadata.
                if let Some(object) = value.as_object_mut() {
                    object.remove("title");
                }
                if let Some(old) = self.schemas.insert(name.clone(), value.clone()) {
                    if old != value {
                        return Err(format!("inconsistent shared schema name: {name}").into());
                    }
                }
            }
        }
        let name = root["title"]
            .as_str()
            .ok_or("named details schema required")?
            .to_owned();
        if root.get("additionalProperties") != Some(&Value::Bool(false)) {
            return Err("domain details must deny extra fields".into());
        }
        let mut fields = Map::new();
        for (key, property) in root["properties"]
            .as_object()
            .ok_or("details properties required")?
        {
            fields.insert(key.clone(), field_contract(property, &self.schemas)?);
        }
        let required = root["required"].clone();
        root.remove("title");
        self.schemas.insert(name.clone(), schema);
        self.roots.insert(name);
        let first: T = serde_json::from_value(seed.clone())?;
        let identity = first.identity();
        let mut categories = Map::new();
        for value in fields[category_field]
            .as_array()
            .ok_or("category field enum required")?
        {
            seed[category_field] = value.clone();
            let details: T = serde_json::from_value(seed.clone())?;
            details.validate()?;
            categories.insert(
                value
                    .as_str()
                    .ok_or("category selector must be a string")?
                    .into(),
                json!(details.identity().category),
            );
        }
        let mut reasons = BTreeSet::new();
        for field in reason_fields {
            for value in fields[*field]
                .as_array()
                .ok_or("reason field enum required")?
            {
                if let Some(reason) = value.as_str() {
                    reasons.insert(reason);
                }
            }
        }
        self.contracts.insert(
            identity.domain.into(),
            json!({
                "version": identity.details_version, "fields": fields, "required": required,
                "category_field": category_field, "categories": categories,
                "reason_fields": reason_fields, "reasons": reasons,
            }),
        );
        Ok(())
    }
}

fn generate() -> Result<String, Box<dyn Error>> {
    let mut artifacts = Artifacts::default();
    artifacts.register::<RepositoryFailureDetails>(
        json!({"operation":"summary", "reason":"unknown_repository"}),
        "reason",
        &["reason"],
    )?;
    artifacts.register::<RepositoryAccessDetails>(
        json!({"operation":"project_access", "reason":"unknown_repository"}),
        "reason",
        &["reason"],
    )?;
    artifacts.register::<workdir::management_failure::WorkdirManagementFailure>(
        json!({"operation":"create", "reason":"working_directory_create_failed"}),
        "reason",
        &["reason"],
    )?;
    artifacts.register::<WorkdirFailureDetails>(json!({"operation":"read", "classification":"denied", "denial_reason":null, "failure_reason":null}), "classification", &["failure_reason", "denial_reason", "classification"])?;
    artifacts.register::<DriveFailureDetails>(
        json!({"operation":"read", "reason":"denied"}),
        "reason",
        &["reason"],
    )?;
    artifacts.register::<ConfigurationFailureDetails>(
        json!({"operation":"source", "reason":"owner_required"}),
        "reason",
        &["reason"],
    )?;
    artifacts.register::<server_api::DriveHttpFailureDetails>(
        json!({"operation":"metadata", "code":"not_found", "classification":"not_committed"}),
        "code",
        &["code"],
    )?;
    artifacts.register::<DriveClientFailureDetails>(
        json!({"operation":"read", "reason":"unavailable"}),
        "reason",
        &["reason"],
    )?;
    artifacts.register::<ConfigFailureDetails>(
        json!({"operation":"read", "stage":"response", "reason":"decode"}),
        "reason",
        &["reason"],
    )?;
    let types = generate_schema_closure_typescript(&artifacts.schemas, &artifacts.roots)?;
    let names = artifacts
        .roots
        .iter()
        .cloned()
        .collect::<Vec<_>>()
        .join(" | ");
    Ok(format!(
        "// @generated from Rust domain cause schemas and FailureCause identity. Do not edit.\n// Regenerate/check: cargo run -q -p yoi-workspace-server --example generate_domain_failure_types -- [--check]\n\n{types}\nexport type DomainFailureDetails = {names};\n\nexport const DOMAIN_FAILURE_CONTRACTS = {} as const;\n",
        serde_json::to_string_pretty(&artifacts.contracts)?
    ))
}

fn main() -> Result<(), Box<dyn Error>> {
    let args: Vec<_> = std::env::args().skip(1).collect();
    if !args.is_empty() && args != ["--check"] {
        return Err("usage: generate_domain_failure_types [--check]".into());
    }
    let output = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../..")
        .join(OUTPUT);
    let generated = generate()?;
    if args.is_empty() {
        fs::write(output, generated)?;
    } else if fs::read_to_string(output)? != generated {
        return Err("domain failure artifact is stale".into());
    }
    Ok(())
}
