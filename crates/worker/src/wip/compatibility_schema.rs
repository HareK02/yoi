//! Conservative JSON-shape-preserving types for the compatibility `input` value.
//! The original JSON Schema remains authoritative and visible in Inspect.
//! In particular, open records and JSON unions must not become closed records
//! or WIP's tagged unions: that would reject/change valid original tool inputs.
use super::{Documentation, Json, TypeExpr};
use wip_protocol::{EnumCase, FieldDeclaration};

pub(super) fn input_type(schema: &Json) -> TypeExpr {
    project(schema, schema, &mut Vec::new(), 0, &mut 2048)
}

fn project(
    schema: &Json,
    root: &Json,
    visiting: &mut Vec<String>,
    depth: usize,
    budget: &mut usize,
) -> TypeExpr {
    if depth >= 16 || *budget == 0 {
        return TypeExpr::Json;
    }
    *budget -= 1;
    let Some(object) = schema.as_object() else {
        return TypeExpr::Json;
    };
    // Composition and resource-relative reference semantics are not equivalent
    // to WIP's wire types. Keep their exact schema visible instead of guessing.
    if [
        "anyOf",
        "oneOf",
        "allOf",
        "$dynamicRef",
        "$recursiveRef",
        "$id",
        "id",
    ]
    .iter()
    .any(|key| object.contains_key(*key))
    {
        return TypeExpr::Json;
    }
    if let Some(reference) = schema.get("$ref").and_then(Json::as_str) {
        let Some(pointer) = reference.strip_prefix('#') else {
            return TypeExpr::Json;
        };
        if (!pointer.is_empty() && !pointer.starts_with('/'))
            || visiting.iter().any(|value| value == reference)
        {
            return TypeExpr::Json;
        }
        let Some(target) = root.pointer(pointer) else {
            return TypeExpr::Json;
        };
        visiting.push(reference.into());
        let result = project(target, root, visiting, depth + 1, budget);
        visiting.pop();
        return result;
    }
    match schema.get("type").and_then(Json::as_str) {
        Some("null") => TypeExpr::Unit,
        Some("boolean") => TypeExpr::Boolean,
        Some("integer") => TypeExpr::Integer,
        Some("number") => TypeExpr::Number,
        Some("string") => {
            if let Some(values) = schema.get("enum").and_then(Json::as_array) {
                if !values.is_empty()
                    && values.len() <= *budget
                    && values.iter().enumerate().all(|(index, value)| {
                        value.as_str().is_some_and(|s| !s.is_empty())
                            && !values[..index].contains(value)
                    })
                {
                    *budget -= values.len();
                    return TypeExpr::Enum {
                        cases: values
                            .iter()
                            .map(|value| EnumCase {
                                name: value.as_str().unwrap().into(),
                                documentation: None,
                            })
                            .collect(),
                    };
                }
            }
            TypeExpr::String
        }
        Some("array") => TypeExpr::List {
            items: Box::new(
                // Neither legacy tuples nor prefixItems describe one uniform element type.
                schema
                    .get("items")
                    .filter(|items| items.is_object() && !object.contains_key("prefixItems"))
                    .map(|items| project(items, root, visiting, depth + 1, budget))
                    .unwrap_or(TypeExpr::Json),
            ),
        },
        Some("object")
            if schema.get("additionalProperties") == Some(&Json::Bool(false))
                && !object.contains_key("patternProperties") =>
        {
            let properties = schema.get("properties").and_then(Json::as_object);
            if properties.is_some_and(|properties| {
                properties.len() > *budget || properties.keys().any(String::is_empty)
            }) {
                return TypeExpr::Json;
            }
            // Reserve sibling fields before expanding descendants: a wide early
            // child must not spend the allocation already promised to siblings.
            *budget -= properties.map_or(0, |properties| properties.len());
            let required = schema.get("required").and_then(Json::as_array);
            TypeExpr::Record {
                fields: properties
                    .into_iter()
                    .flat_map(|properties| properties.iter())
                    .map(|(name, value)| FieldDeclaration {
                        name: name.clone(),
                        required: required.is_some_and(|required| {
                            required.iter().any(|field| field.as_str() == Some(name))
                        }),
                        documentation: value.get("description").and_then(Json::as_str).map(
                            |summary| Documentation {
                                summary: summary.into(),
                                details: None,
                            },
                        ),
                        r#type: project(value, root, visiting, depth + 1, budget),
                    })
                    .collect(),
            }
        }
        _ => TypeExpr::Json,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn closed_records_lists_required_fields_and_enum_are_not_erased() {
        let schema = json!({"type":"object","additionalProperties":false,"required":["items"],"properties":{
            "items":{"type":"array","items":{"$ref":"#/$defs/Item"}},
            "limit":{"type":"integer","minimum":1},
            "enabled":{"type":"boolean"}
        },"$defs":{"Item":{"type":"object","additionalProperties":false,"required":["kind"],"properties":{"kind":{"type":"string","enum":["file","folder"]},"weight":{"type":"number"}}}}});
        let TypeExpr::Record { fields } = input_type(&schema) else {
            panic!("record lost")
        };
        let items = fields.iter().find(|field| field.name == "items").unwrap();
        assert!(items.required);
        let TypeExpr::List { items } = &items.r#type else {
            panic!("list lost")
        };
        let TypeExpr::Record { fields: nested } = items.as_ref() else {
            panic!("local ref lost")
        };
        assert!(matches!(nested[0].r#type, TypeExpr::Enum { .. }));
        assert_eq!(
            fields
                .iter()
                .find(|field| field.name == "limit")
                .unwrap()
                .r#type,
            TypeExpr::Integer
        );
        assert!(
            !fields
                .iter()
                .find(|field| field.name == "limit")
                .unwrap()
                .required
        );
    }

    #[test]
    fn unrepresentable_json_shapes_remain_json_not_invented_tagged_values() {
        for schema in [
            json!({"type":"object","properties":{"known":{"type":"string"}}}),
            json!({"type":"object","additionalProperties":{"type":"integer"}}),
            json!({"type":["string","null"]}),
            json!({"anyOf":[{"type":"string"},{"type":"null"}]}),
            json!({"oneOf":[{"type":"string"},{"type":"integer"}]}),
            json!({"allOf":[{"type":"object"}]}),
            json!({"$ref":"#"}),
            json!({"$ref":"https://example.invalid/schema"}),
            json!({"type":"object","additionalProperties":false,"patternProperties":{"^x":{"type":"string"}}}),
        ] {
            assert_eq!(input_type(&schema), TypeExpr::Json, "{schema}");
        }
        assert_eq!(
            input_type(&json!({"type":"array","items":[{"type":"string"},{"type":"integer"}]})),
            TypeExpr::List {
                items: Box::new(TypeExpr::Json)
            }
        );
    }

    #[test]
    fn acyclic_reference_expansion_is_bounded_before_allocating_siblings() {
        let mut definitions = serde_json::Map::new();
        for depth in 0..12 {
            let properties: serde_json::Map<String, Json> = (0..40)
                .map(|index| {
                    (
                        format!("field{index}"),
                        if depth == 11 {
                            json!({"type":"string"})
                        } else {
                            json!({"$ref":format!("#/$defs/Level{}", depth + 1)})
                        },
                    )
                })
                .collect();
            definitions.insert(
                format!("Level{depth}"),
                json!({"type":"object","additionalProperties":false,"properties":properties}),
            );
        }
        let projected = input_type(&json!({"$ref":"#/$defs/Level0","$defs":definitions}));
        fn nodes(ty: &TypeExpr) -> usize {
            match ty {
                TypeExpr::Record { fields } => {
                    1 + fields
                        .iter()
                        .map(|field| 1 + nodes(&field.r#type))
                        .sum::<usize>()
                }
                TypeExpr::List { items } => 1 + nodes(items),
                _ => 1,
            }
        }
        assert!(nodes(&projected) < 4096);
        assert!(matches!(projected, TypeExpr::Record { .. }));
    }

    #[test]
    fn recursive_refs_terminate_without_losing_outer_fields() {
        let schema = json!({"$ref":"#/$defs/Node","$defs":{"Node":{"type":"object","additionalProperties":false,"properties":{"next":{"$ref":"#/$defs/Node"}}}}});
        let TypeExpr::Record { fields } = input_type(&schema) else {
            panic!("outer record lost")
        };
        assert_eq!(fields[0].r#type, TypeExpr::Json);
    }
}

#[cfg(test)]
mod integration_tests {
    use super::super::*;
    use serde_json::json;
    use std::sync::atomic::{AtomicUsize, Ordering};

    struct EchoInput(Arc<AtomicUsize>);
    #[async_trait]
    impl Tool for EchoInput {
        async fn execute(
            &self,
            input: &str,
            _: ToolExecutionContext,
        ) -> Result<ToolOutput, ToolError> {
            self.0.fetch_add(1, Ordering::SeqCst);
            Ok(ToolOutput {
                summary: "Echo input".into(),
                content: Some(input.into()),
                attachments: Vec::new(),
            })
        }
    }
    fn runtime(schema: Json) -> (WipRuntime, Arc<AtomicUsize>) {
        let calls = Arc::new(AtomicUsize::new(0));
        let projection = compatibility_projection(
            ToolMeta::new("Fixture")
                .description("Typed compatibility fixture")
                .input_schema(schema),
            Arc::new(EchoInput(calls.clone())),
            None,
        )
        .unwrap();
        let mut registry = WipMountRegistry::new();
        registry.mount(projection).unwrap();
        (
            WipRuntime::from_mounts(registry, "schema-test".into()).unwrap(),
            calls,
        )
    }
    async fn invoke(runtime: &WipRuntime, input: Json) -> Result<ToolOutput, ToolError> {
        runtime
            .invoke(
                "/tools/Fixture".into(),
                root_reference("yoi.tool/Fixture/v1"),
                "call".into(),
                json!({"input":input}),
                ToolExecutionContext::direct(),
            )
            .await
    }

    #[tokio::test]
    async fn inspect_exposes_nested_types_and_exact_constraints_and_invoke_preserves_json() {
        let schema = json!({
            "type":"object", "additionalProperties":false, "required":["items","mode","nullable","attributes"],
            "properties":{
                "items":{"type":"array","minItems":1,"items":{"$ref":"#/$defs/Item"}},
                "mode":{"type":"string","enum":["fast","safe"]},
                "limit":{"type":"integer","minimum":1,"maximum":10,"description":"Bounded count"},
                "nullable":{"anyOf":[{"type":"string"},{"type":"null"}]},
                "attributes":{"type":"object","additionalProperties":{"type":"integer"}}
            },
            "$defs":{"Item":{"type":"object","additionalProperties":false,"required":["name"],"properties":{"name":{"type":"string","minLength":2,"pattern":"^[a-z]+$"}}}}
        });
        let (runtime, calls) = runtime(schema.clone());
        let output = runtime
            .inspect("/::\"yoi.tool/Fixture/v1\"".into(), false)
            .await
            .unwrap();
        let inspected: Json = serde_json::from_str(output.content.as_deref().unwrap()).unwrap();
        let signature = inspected["interface_signature"].as_str().unwrap();
        for text in [
            "items",
            "mode",
            "enum",
            "limit?: integer",
            "Bounded count",
            "nullable",
            "anyOf",
            "attributes",
            "additionalProperties",
            "minItems",
            "minimum",
            "maximum",
            "minLength",
            "pattern",
            "$defs",
        ] {
            assert!(signature.contains(text), "missing {text}: {signature}");
        }
        // Full schema is in the rendered summary, not exclusively hidden details.
        let escaped_schema = serde_json::to_string(&schema.to_string()).unwrap();
        assert!(signature.contains(&escaped_schema[1..escaped_schema.len() - 1]));
        assert_eq!(runtime.metrics().discover_round_trips, 0);
        let good = json!({"items":[{"name":"entry"}],"mode":"safe","nullable":null,"attributes":{"arbitrary-key":4},"limit":3});
        let output = invoke(&runtime, good.clone()).await.unwrap();
        assert_eq!(
            serde_json::from_str::<Json>(output.content.as_deref().unwrap()).unwrap(),
            good
        );
        assert_eq!(calls.load(Ordering::SeqCst), 1);
        for invalid in [
            json!({"items":[{"name":"x"}],"mode":"safe","nullable":null,"attributes":{}}),
            json!({"items":[{"name":"Entry"}],"mode":"safe","nullable":null,"attributes":{}}),
            json!({"items":[],"mode":"safe","nullable":null,"attributes":{}}),
            json!({"items":[{"name":"entry"}],"mode":"fast","nullable":null,"attributes":{},"limit":11}),
            json!({"items":[{"name":"entry"}],"mode":"other","nullable":null,"attributes":{}}),
            json!({"items":[{"name":"entry"}],"mode":"safe","attributes":{}}),
            json!({"items":[{"name":"entry"}],"mode":"safe","nullable":false,"attributes":{}}),
            json!({"items":[{"name":"entry"}],"mode":"safe","nullable":null,"attributes":{"x":"wrong"}}),
        ] {
            assert!(invoke(&runtime, invalid).await.is_err());
        }
        assert_eq!(
            calls.load(Ordering::SeqCst),
            1,
            "invalid inputs must not reach the original tool"
        );
    }

    #[tokio::test]
    async fn open_records_nullable_unions_and_recursive_inputs_keep_their_original_shape() {
        for (schema, input) in [
            (
                json!({"type":"object","properties":{"known":{"type":"string"}}}),
                json!({"known":"yes","extra":[1,null,{"free":true}]}),
            ),
            (json!({"type":["string","null"]}), Json::Null),
            (json!({"type":"null"}), Json::Null),
            (json!({"type":"boolean"}), json!(true)),
            (json!({"type":"number"}), json!(1.5)),
            (json!({"type":"string","enum":["", "value"]}), json!("")),
            (
                json!({"type":"array","prefixItems":[{"type":"string"}],"items":{"type":"integer"}}),
                json!(["first", 2]),
            ),
            (
                json!({"type":"object","additionalProperties":false,"properties":{"defaulted":{"type":"string","default":"do not insert"}}}),
                json!({}),
            ),
            (
                json!({"$ref":"#/$defs/A~1B","$defs":{"A/B":{"type":"string"}}}),
                json!("escaped pointer"),
            ),
            (
                json!({"oneOf":[{"type":"integer"},{"type":"string"}]}),
                json!(7),
            ),
            (
                json!({"$ref":"#/$defs/Node","$defs":{"Node":{"type":"object","additionalProperties":false,"properties":{"next":{"$ref":"#/$defs/Node"}}}}}),
                json!({"next":{"next":{}}}),
            ),
        ] {
            let (runtime, calls) = runtime(schema);
            let output = invoke(&runtime, input.clone()).await.unwrap();
            assert_eq!(
                serde_json::from_str::<Json>(output.content.as_deref().unwrap()).unwrap(),
                input
            );
            assert_eq!(calls.load(Ordering::SeqCst), 1);
        }
    }
}
