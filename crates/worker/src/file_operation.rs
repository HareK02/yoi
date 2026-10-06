//! WIP presentation of the provider-neutral file argument contracts. Providers
//! choose which read surface to publish; routing and preconditions are not args.
use fs_operation::text::TextOperation;
use wip_protocol::{ParameterDeclaration, TypeExpr};

pub(crate) fn parameters(name: &str, line_read: bool) -> Option<Vec<ParameterDeclaration>> {
    let schema = TextOperation::from_name(name, line_read)?.argument_schema();
    Some(schema_parameters(&schema))
}

fn schema_parameters(schema: &schemars::Schema) -> Vec<ParameterDeclaration> {
    let schema = schema.as_value();
    let required = schema["required"].as_array();
    schema["properties"]
        .as_object()
        .into_iter()
        .flat_map(|properties| properties.iter())
        .map(|(name, property)| {
            // These core arguments are primitives. Optional Tool read fields
            // accept null; WIP keeps its existing optional integer declaration.
            let kind = property["type"].as_str().or_else(|| {
                property["type"]
                    .as_array()?
                    .iter()
                    .filter_map(|v| v.as_str())
                    .find(|t| *t != "null")
            });
            let r#type = match kind {
                Some("string") => TypeExpr::String,
                Some("boolean") => TypeExpr::Boolean,
                Some("integer") => TypeExpr::Integer,
                _ => panic!("file operation arguments must be primitive: {name}"),
            };
            ParameterDeclaration {
                name: name.clone(),
                required: required
                    .is_some_and(|items| items.iter().any(|item| item.as_str() == Some(name))),
                documentation: None,
                r#type,
            }
        })
        .collect()
}
