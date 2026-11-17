//! Editor-only Decodal schemas derived from the actual Profile input types.
//!
//! No defaults are materialized here and runtime decoding remains Serde. The
//! bridge handles Rust's optional/container types that Decodal 0.4's derive does
//! not implement; adding a field with an unsupported type is a compile error.

use decodal::{Constraint, PrimitiveType, Value};
use std::{
    collections::{BTreeMap, HashMap},
    marker::PhantomData,
    num::NonZeroU32,
    path::PathBuf,
};

pub(crate) trait ConfigSchema {
    fn config_schema() -> Value;
}

/// Schema-only field in a generated `#[derive(decodal::Decodal)]` declaration.
/// Never used to decode production data or materialize configuration defaults.
pub(crate) struct SchemaField<T>(PhantomData<T>);
impl<T: ConfigSchema> decodal::DecodalSchema for SchemaField<T> {
    fn decodal_schema() -> Value {
        T::config_schema()
    }
}
impl<T> decodal::DecodalDecode for SchemaField<T> {
    fn decodal_decode(_: &decodal::Data) -> decodal::DecodeResult<Self> {
        Err(decodal::DecodeError::new(
            "",
            "authoring schema cannot decode configuration",
        ))
    }
}

macro_rules! primitive {
    ($method:ident: $($ty:ty),+ $(,)?) => {$(
        impl ConfigSchema for $ty {
            fn config_schema() -> Value { Value::$method() }
        }
    )+};
}
primitive!(string_type: String, PathBuf, crate::SymlinkPolicy);
primitive!(bool_type: bool);
primitive!(int_type: u32, u64, usize, i64, NonZeroU32);
// Decodal has no numeric union. Serde checks float/int inputs at the save boundary.
primitive!(unknown: f32, f64, serde_json::Value, crate::model::ReasoningControl, crate::model::ModelCapability);

impl<T: ConfigSchema> ConfigSchema for Option<T> {
    fn config_schema() -> Value {
        T::config_schema()
    }
}
impl<T: ConfigSchema> ConfigSchema for Vec<T> {
    fn config_schema() -> Value {
        Value::array_of(T::config_schema())
    }
}
impl<T: ConfigSchema> ConfigSchema for BTreeMap<String, T> {
    fn config_schema() -> Value {
        Value::map_of(T::config_schema())
    }
}
impl<T: ConfigSchema> ConfigSchema for HashMap<String, T> {
    fn config_schema() -> Value {
        Value::map_of(T::config_schema())
    }
}

/// Approximate a Serde union for editor constraints, retaining all known fields.
/// Different ranges for a shared name require Unknown (Decodal has no union).
/// Neither this approximation nor editor completion replaces runtime validation.
pub(crate) fn merge_shapes(shapes: impl IntoIterator<Item = Value>) -> Value {
    let mut result = BTreeMap::new();
    for shape in shapes {
        let Value::Object { fields, rest: None } = shape else {
            panic!("schema union members must be closed objects");
        };
        for field in fields {
            result
                .entry(field.name)
                .and_modify(|value| {
                    if *value != field.value {
                        *value = Value::unknown();
                    }
                })
                .or_insert(field.value);
        }
    }
    Value::object(result)
}

/// Encode the generated shape in the existing saved schema-source contract.
/// This is serialization, not a second list of Profile fields. Keep persisted
/// fingerprints and browser/wasm evaluation bound to the exact generated source.
pub(crate) fn source(value: Value) -> String {
    fn defaulted(base: String, default: Option<Box<Value>>) -> String {
        match default {
            Some(value) => format!("({base}) default ({})", source(*value)),
            None => base,
        }
    }
    match value {
        Value::String(value) => serde_json::to_string(&value).expect("string serializes"),
        Value::Int(value) => value.to_string(),
        Value::Float(value) => serde_json::to_string(&value).expect("finite schema number"),
        Value::Bool(value) => value.to_string(),
        Value::Array(values) => format!(
            "[{}]",
            values.into_iter().map(source).collect::<Vec<_>>().join(",")
        ),
        Value::Object { fields, rest } => {
            let mut fields = fields
                .into_iter()
                .map(|field| {
                    assert!(
                        field.name.chars().enumerate().all(|(i, c)| c == '_'
                            || c.is_ascii_alphabetic()
                            || (i > 0 && c.is_ascii_digit())),
                        "schema field must be a Decodal identifier"
                    );
                    format!("{} = {};", field.name, source(field.value))
                })
                .collect::<Vec<_>>();
            if let Some(rest) = rest {
                fields.push(format!("...{}", source(*rest)));
            }
            format!("{{ {} }}", fields.join(" "))
        }
        Value::ArrayRange {
            item,
            constraints,
            default,
        } => {
            assert!(constraints.is_empty(), "unsupported array constraints");
            defaulted(format!("[...{}]", source(*item)), default)
        }
        Value::MapRange {
            value,
            constraints,
            default,
        } => {
            assert!(constraints.is_empty(), "unsupported map constraints");
            defaulted(format!("{{ ...{} }}", source(*value)), default)
        }
        Value::Range {
            constraints,
            default,
        } => {
            let ranges = constraints
                .into_iter()
                .map(|constraint| match constraint {
                    Constraint::Unknown => "Unknown",
                    Constraint::Type(PrimitiveType::String) => "String",
                    Constraint::Type(PrimitiveType::Int) => "Int",
                    Constraint::Type(PrimitiveType::Float) => "Float",
                    Constraint::Type(PrimitiveType::Bool) => "Bool",
                    _ => panic!("unsupported generated authoring constraint"),
                })
                .collect::<Vec<_>>();
            assert!(!ranges.is_empty());
            defaulted(ranges.join(" & "), default)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[derive(serde::Deserialize, config_schema_derive::ConfigSchema)]
    #[allow(dead_code)]
    struct EvolvingConfig {
        #[serde(rename = "ref", alias = "reference")]
        source: Option<String>,
        newly_added: Option<Vec<BTreeMap<String, bool>>>,
    }

    #[test]
    fn derived_fields_names_aliases_and_containers_round_trip_without_a_field_registry() {
        let shape = EvolvingConfig::config_schema();
        assert_eq!(
            shape,
            Value::object([
                ("ref", Value::string_type()),
                ("reference", Value::string_type()),
                (
                    "newly_added",
                    Value::array_of(Value::map_of(Value::bool_type()))
                ),
            ])
        );
        let mut engine = decodal::Engine::new(decodal::EmptyLoader);
        let module = engine
            .add_root_source("schema", "schema", &source(shape.clone()))
            .unwrap();
        let generated = engine.eval_module(module).unwrap();
        engine.bind_global_runtime("Generated", generated);
        engine.bind_global("Derived", shape).unwrap();
        let module = engine
            .add_root_source(
                "value",
                "value",
                "{ ref = \"x\"; newly_added = [{ flag = false; }]; } as Generated as Derived",
            )
            .unwrap();
        let value = engine.eval_module(module).unwrap();
        // Partial values can be analyzed but not materialized from an editor
        // shape: absent aliases/fields stay abstract instead of acquiring defaults.
        assert!(engine.materialize(&value).is_err());
        let module = engine
            .add_root_source("empty", "empty", "{} as Generated")
            .unwrap();
        let value = engine.eval_module(module).unwrap();
        assert!(engine.materialize(&value).is_err());
    }

    #[test]
    fn generated_profile_source_is_deterministic_and_has_no_removed_permissions() {
        let first = crate::profile_authoring_schema_source();
        assert_eq!(first, crate::profile_authoring_schema_source());
        assert!(!first.contains("permissions ="));
        for name in ["filesystem", "filesystem_write", "bash", "orchestration"] {
            assert!(first.contains(&format!("{name} =")), "{name}");
        }
    }
}
