//! Offline schema admission shared by package consumers. No guest, DCDL evaluator,
//! import loader, or ambient JSON Schema retriever participates in validation.

use std::collections::{BTreeMap, BTreeSet};

use decodal::{Ast, Expr, ExprId, Literal, SyntaxTokenKind as Token};
use serde_json::Value;

const MAX_SCHEMA_BYTES: usize = 64 * 1024;
const MAX_DEPTH: usize = 32;
const MAX_NODES: usize = 4096;
const TOOL_DIALECT: &str = "https://json-schema.org/draft/2020-12/schema";

/// Admit a bounded, offline JSON Schema 2020-12 document (the default dialect).
///
/// Unknown keywords/dialects, resource IDs, dynamic/anchor/external references,
/// dangling references and recursive reference graphs are unsupported and rejected.
/// Local JSON Pointer references must target schema positions, not annotation data.
/// This checks the schema, not a Tool payload; callers must still validate payloads.
pub fn validate_tool_schema(schema: &Value) -> Result<(), String> {
    bound_json(schema)?;
    let mut graph = BTreeMap::new();
    inspect_tool_schema(schema, "", &mut graph)?;
    let mut complete = BTreeMap::new();
    for path in graph.keys() {
        check_reference_graph(path, &graph, &mut BTreeSet::new(), &mut complete, 0)?;
    }
    // Use the fixed built-in meta-schema, never schema-directed dialect retrieval.
    jsonschema::draft202012::meta::validate(schema)
        .map_err(|error| format!("invalid Tool JSON Schema: {error}"))?;
    // The compiler normally skips unreferenced $defs. Compile every admitted
    // schema position through one in-memory resource, so invalid regexes/formats
    // cannot hide in an unused definition. These URNs are generated, not input.
    const RESOURCE: &str = "urn:yoi:plugin:tool-schema";
    let resource = jsonschema::Resource::from_contents(schema.clone())
        .map_err(|error| format!("invalid Tool JSON Schema resource: {error}"))?;
    let all_schemas = serde_json::json!({
        "allOf": graph.keys().map(|path| {
            serde_json::json!({"$ref": format!("{RESOURCE}#{}", uri_fragment(path))})
        }).collect::<Vec<_>>()
    });
    jsonschema::options()
        .with_draft(jsonschema::Draft::Draft202012)
        .with_resource(RESOURCE, resource)
        .with_retriever(NoRetrieval)
        .should_validate_formats(true)
        .should_ignore_unknown_formats(false)
        .build(&all_schemas)
        .map_err(|error| format!("invalid Tool JSON Schema: {error}"))?;
    Ok(())
}

struct NoRetrieval;

impl jsonschema::Retrieve for NoRetrieval {
    fn retrieve(
        &self,
        _uri: &jsonschema::Uri<String>,
    ) -> Result<Value, Box<dyn std::error::Error + Send + Sync>> {
        Err("external schema retrieval is forbidden".into())
    }
}

fn bound_json(value: &Value) -> Result<(), String> {
    let mut stack = vec![(value, 0)];
    let mut count = 0;
    while let Some((value, depth)) = stack.pop() {
        count += 1;
        if count > MAX_NODES || depth > MAX_DEPTH {
            return Err("Tool JSON Schema exceeds static complexity limits".into());
        }
        match value {
            Value::Array(values) => stack.extend(values.iter().map(|v| (v, depth + 1))),
            Value::Object(values) => stack.extend(values.values().map(|v| (v, depth + 1))),
            _ => {}
        }
    }
    if serde_json::to_vec(value).map_err(|e| e.to_string())?.len() > MAX_SCHEMA_BYTES {
        return Err("Tool JSON Schema exceeds static byte limit".into());
    }
    Ok(())
}

fn pointer_child(parent: &str, key: &str) -> String {
    format!("{parent}/{}", key.replace('~', "~0").replace('/', "~1"))
}

fn uri_fragment(pointer: &str) -> String {
    let mut encoded = String::new();
    const HEX: &[u8; 16] = b"0123456789ABCDEF";
    for byte in pointer.bytes() {
        if byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'.' | b'_' | b'~' | b'/') {
            encoded.push(char::from(byte));
        } else {
            encoded.push('%');
            encoded.push(char::from(HEX[usize::from(byte >> 4)]));
            encoded.push(char::from(HEX[usize::from(byte & 15)]));
        }
    }
    encoded
}

fn inspect_tool_schema(
    schema: &Value,
    path: &str,
    graph: &mut BTreeMap<String, Vec<String>>,
) -> Result<(), String> {
    graph.insert(path.into(), Vec::new());
    if schema.is_boolean() {
        return Ok(());
    }
    let object = schema
        .as_object()
        .ok_or_else(|| format!("schema at #{path} must be an object or boolean"))?;
    let mut children = Vec::new();
    let mut references = Vec::new();
    for (key, value) in object {
        let location = pointer_child(path, key);
        match key.as_str() {
            "$schema" => {
                if !path.is_empty() || value.as_str() != Some(TOOL_DIALECT) {
                    return Err(format!("unsupported Tool schema dialect at #{location}"));
                }
            }
            "$ref" => {
                let reference = value
                    .as_str()
                    .ok_or_else(|| format!("$ref at #{location} must be a string"))?;
                let target = reference.strip_prefix('#').ok_or_else(|| {
                    format!("external Tool schema reference at #{location} is forbidden")
                })?;
                if (!target.is_empty() && !target.starts_with('/')) || target.contains('%') {
                    return Err(format!(
                        "only local JSON Pointer $ref is supported at #{location}"
                    ));
                }
                references.push(target.to_owned());
            }
            "$defs" | "properties" | "patternProperties" | "dependentSchemas" => {
                let entries = value
                    .as_object()
                    .ok_or_else(|| format!("schema map at #{location} must be an object"))?;
                for (name, child) in entries {
                    children.push((pointer_child(&location, name), child));
                }
            }
            "allOf" | "anyOf" | "oneOf" | "prefixItems" => {
                let entries = value
                    .as_array()
                    .ok_or_else(|| format!("schema list at #{location} must be an array"))?;
                for (index, child) in entries.iter().enumerate() {
                    children.push((pointer_child(&location, &index.to_string()), child));
                }
            }
            "items"
            | "contains"
            | "additionalProperties"
            | "unevaluatedProperties"
            | "unevaluatedItems"
            | "propertyNames"
            | "not"
            | "if"
            | "then"
            | "else" => {
                children.push((location, value));
            }
            // These contain data, not subschemas. A property name or literal value
            // resembling a keyword must not acquire schema semantics.
            "type" | "enum" | "const" | "multipleOf" | "maximum" | "exclusiveMaximum"
            | "minimum" | "exclusiveMinimum" | "maxLength" | "minLength" | "pattern" | "format"
            | "maxItems" | "minItems" | "uniqueItems" | "maxContains" | "minContains"
            | "maxProperties" | "minProperties" | "required" | "dependentRequired" | "title"
            | "description" | "default" | "examples" | "deprecated" | "readOnly" | "writeOnly"
            | "$comment" => {}
            _ => return Err(format!("unsupported Tool schema keyword at #{location}")),
        }
    }
    references.extend(children.iter().map(|(path, _)| path.clone()));
    graph.insert(path.into(), references);
    for (path, child) in children {
        inspect_tool_schema(child, &path, graph)?;
    }
    Ok(())
}

fn check_reference_graph(
    path: &str,
    graph: &BTreeMap<String, Vec<String>>,
    active: &mut BTreeSet<String>,
    complete: &mut BTreeMap<String, usize>,
    depth: usize,
) -> Result<usize, String> {
    if active.contains(path) {
        return Err(format!(
            "recursive Tool schema reference at #{path} is unsupported"
        ));
    }
    if depth > MAX_DEPTH {
        return Err("Tool schema reference graph exceeds static depth limit".into());
    }
    let children = graph
        .get(path)
        .ok_or_else(|| format!("Tool schema reference does not target a schema: #{path}"))?;
    if let Some(height) = complete.get(path) {
        if depth + height > MAX_DEPTH {
            return Err("Tool schema reference graph exceeds static depth limit".into());
        }
        return Ok(*height);
    }
    active.insert(path.into());
    let mut height = 0;
    for child in children {
        height = height.max(1 + check_reference_graph(child, graph, active, complete, depth + 1)?);
    }
    active.remove(path);
    complete.insert(path.into(), height);
    Ok(height)
}

/// Admit a closed static subset of actual DCDL syntax using decodal's parser/AST.
///
/// The root must be an object. Nested closed objects, homogeneous array constraints
/// (`[...String]`), primitive types (`String`, `Bool`, `Int`, `Float`) and scalar
/// singleton literals are supported. Object fields must have unique single-part
/// paths. Imports, open objects/maps, Unknown, defaults, references, expressions,
/// calls and user functions are rejected, not evaluated. This does not validate a
/// config instance or establish that its values contain no secrets.
pub fn validate_config_schema(source: &str) -> Result<(), String> {
    if source.len() > MAX_SCHEMA_BYTES {
        return Err("DCDL config schema exceeds static byte limit".into());
    }
    let tokens = decodal::tokenize_source(source)
        .map_err(|error| format!("invalid DCDL config schema: {}", error.message))?;
    if tokens.len() > MAX_NODES {
        return Err("DCDL config schema exceeds static token limit".into());
    }
    // Guard parser recursion before parsing untrusted source. Forbid operators
    // outside the subset here as well, so long right-associative expressions
    // cannot consume the parser stack before the AST admission check.
    let mut depth = 0usize;
    for (index, token) in tokens.iter().enumerate() {
        match &token.kind {
            Token::LBrace | Token::LBracket | Token::LParen => {
                depth += 1;
                if depth > MAX_DEPTH {
                    return Err("DCDL config schema exceeds static depth limit".into());
                }
            }
            Token::RBrace | Token::RBracket | Token::RParen => {
                depth = depth.saturating_sub(1);
            }
            Token::Minus
                if tokens
                    .get(index + 1)
                    .is_some_and(|next| matches!(next.kind, Token::Int(_) | Token::Float(_))) => {}
            Token::Ident(_)
            | Token::Int(_)
            | Token::Float(_)
            | Token::String(_)
            | Token::True
            | Token::False
            | Token::Semicolon
            | Token::Comma
            | Token::Ellipsis
            | Token::Colon
            | Token::Equal
            | Token::Comment
            | Token::Eof => {}
            _ => return Err("unsupported expression in static DCDL config schema".into()),
        }
    }
    let parsed = decodal::parse_source(source)
        .map_err(|error| format!("invalid DCDL config schema: {}", error.message))?;
    let root = unparenthesized(&parsed.ast, parsed.root);
    if !matches!(parsed.ast.get(root).expr, Expr::Object { .. }) {
        return Err("DCDL config schema root must be a closed object".into());
    }
    inspect_config_schema(&parsed.ast, root)
}

fn unparenthesized(ast: &Ast, mut id: ExprId) -> ExprId {
    while let Expr::Parenthesized { expr } = ast.get(id).expr {
        id = expr;
    }
    id
}

fn inspect_config_schema(ast: &Ast, id: ExprId) -> Result<(), String> {
    match &ast.get(unparenthesized(ast, id)).expr {
        Expr::Object { fields, rest } => {
            if rest.is_some() {
                return Err(
                    "DCDL config schema objects must be closed (no rest constraint)".into(),
                );
            }
            let mut names = BTreeSet::new();
            for field in fields {
                if field.path.len() != 1 || !names.insert(&field.path[0]) {
                    return Err("DCDL config schema requires unique single-part field paths".into());
                }
                inspect_config_schema(ast, field.value)?;
            }
            Ok(())
        }
        Expr::ArrayConstraint { item } => inspect_config_schema(ast, *item),
        Expr::Ident(name) if matches!(name.as_str(), "String" | "Bool" | "Int" | "Float") => Ok(()),
        Expr::Literal(Literal::Float(value)) if !value.is_finite() => {
            Err("DCDL config schema requires finite scalar literals".into())
        }
        Expr::Literal(_) => Ok(()),
        Expr::Unary {
            op: decodal::ast::UnaryOp::Neg,
            expr,
        } if matches!(ast.get(*expr).expr, Expr::Literal(Literal::Int(_)))
            || matches!(ast.get(*expr).expr, Expr::Literal(Literal::Float(v)) if v.is_finite()) =>
        {
            Ok(())
        }
        _ => Err("unsupported node in closed static DCDL config schema".into()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn tool_schema_accepts_typed_objects_and_local_definitions() {
        let schema = json!({
            "$schema": TOOL_DIALECT,
            "$defs": {"entry/type": {"type": "string", "minLength": 1}},
            "type": "object",
            "properties": {
                "label": {"$ref": "#/$defs/entry~1type"},
                "values": {"type": "array", "items": {"type": "integer"}}
            },
            "required": ["label"],
            "additionalProperties": false
        });
        validate_tool_schema(&schema).unwrap();
        for schema in [json!({}), json!(true), json!(false)] {
            validate_tool_schema(&schema).unwrap();
        }
    }

    #[test]
    fn tool_schema_rejects_invalid_keyword_shapes_and_types() {
        for schema in [
            json!(null),
            json!([]),
            json!({"type": "unknown"}),
            json!({"required": "label"}),
            json!({"properties": []}),
            json!({"properties": {"nested": {"type": 3}}}),
            json!({"allOf": []}),
            json!({"items": []}),
            json!({"minLength": -1}),
            json!({"pattern": "["}),
            json!({"required": ["x", "x"]}),
            json!({"format": "unknown-format"}),
        ] {
            assert!(validate_tool_schema(&schema).is_err(), "accepted {schema}");
        }
    }

    #[test]
    fn tool_schema_rejects_unknown_dialects_keywords_and_nested_extensions() {
        for schema in [
            json!({"$schema": "https://example.invalid/schema"}),
            json!({"$schema": "http://json-schema.org/draft-07/schema#"}),
            json!({"typo": true}),
            json!({"$id": "https://example.invalid/base"}),
            json!({"$dynamicRef": "#node"}),
            json!({"$anchor": "node"}),
            json!({"$defs": {"unused": {"typo": true}}}),
            json!({"properties": {"x": {"$schema": TOOL_DIALECT}}}),
            json!({"contentSchema": {"type": "string"}}),
        ] {
            assert!(validate_tool_schema(&schema).is_err(), "accepted {schema}");
        }
    }

    #[test]
    fn tool_schema_rejects_external_dangling_and_recursive_references() {
        for reference in [
            "https://example.invalid/schema",
            "file:///etc/passwd",
            "other.json",
            "//example.invalid/schema",
            "urn:example:schema",
            "#missing",
            "#/$defs/missing",
            "#",
            "#/%24defs/x",
        ] {
            assert!(
                validate_tool_schema(&json!({"$ref": reference})).is_err(),
                "{reference}"
            );
        }
        for schema in [
            json!({"$defs": {"unused": {"$ref": "https://example.invalid/schema"}}}),
            json!({"default": {"type": "string"}, "$ref": "#/default"}),
            json!({"$defs": {"a": {"$ref": "#/$defs/b"}, "b": {"$ref": "#/$defs/a"}}}),
        ] {
            assert!(validate_tool_schema(&schema).is_err(), "accepted {schema}");
        }
    }

    #[test]
    fn tool_schema_rejects_invalid_unreferenced_definitions() {
        for schema in [
            json!({"$defs": {"unused": {"pattern": "["}}}),
            json!({"$defs": {"unused": {"format": "unknown-format"}}}),
            json!({"$defs": {"unused": {"properties": {"x": {"pattern": "["}}}}}),
        ] {
            assert!(validate_tool_schema(&schema).is_err(), "accepted {schema}");
        }
    }

    #[test]
    fn tool_schema_treats_annotations_and_property_names_as_data() {
        validate_tool_schema(&json!({
            "properties": {
                "$ref": {"type": "string"},
                "unknown": {"const": {"typo": 1}},
                "space # % ~ / 日本語": {"type": "string"}
            },
            "default": {"$ref": "https://example.invalid/data", "typo": 3},
            "examples": [{"$schema": "not a schema"}]
        }))
        .unwrap();
    }

    #[test]
    fn config_schema_accepts_closed_dcdl_types_and_singletons() {
        for source in [
            "{}",
            "label = String; enabled = Bool; retries = Int; ratio = Float",
            "{ nested = { label = String }; tags = [...String]; rows = [...{ id = Int }] }",
            "{ mode = \"safe\"; enabled = true; count = 3; ratio = 0.5; offset = -2; delta = -1.5 }",
            "# DCDL comment\n { label = (String) }",
        ] {
            validate_config_schema(source).unwrap_or_else(|error| panic!("{source}: {error}"));
        }
    }

    #[test]
    fn config_schema_rejects_open_untyped_and_dynamic_dcdl() {
        for source in [
            "Unknown",
            "String",
            "[...String]",
            "{ x = Unknown }",
            "{ ...String }",
            "{ x = String; ...Unknown }",
            "{ nested = { ...Int } }",
            "{ x = [...Unknown] }",
            "import \"remote.dcdl\"",
            "{ x = import \"/etc/passwd\" }",
            "let T = String in { x = T }",
            "{ x = MissingType }",
            "{ x = env(\"TOKEN\") }",
            "{ x = String default \"hello\" }",
            "{ x = (v) => v }",
            "{ x = 1 + 1 }",
            "{ x = [] }",
            "{ x = [String] }",
            "{ x = _ }",
            "{ x = String; x = Int }",
            "{ x.y = String }",
            "{ x = String",
            "{\"type\": \"object\"}",
            "{ x = ---1 }",
        ] {
            assert!(validate_config_schema(source).is_err(), "accepted {source}");
        }
    }

    #[test]
    fn schemas_reject_oversize_and_deep_inputs_without_evaluation() {
        assert!(validate_config_schema(&" ".repeat(MAX_SCHEMA_BYTES + 1)).is_err());
        assert!(
            validate_config_schema(&format!("{}String{}", "(".repeat(1000), ")".repeat(1000)))
                .is_err()
        );
        assert!(
            validate_tool_schema(&json!({"description": "x".repeat(MAX_SCHEMA_BYTES)})).is_err()
        );
        let mut nested = json!({});
        for _ in 0..MAX_DEPTH + 1 {
            nested = json!({"not": nested});
        }
        assert!(validate_tool_schema(&nested).is_err());
        // Each suffix is visited and cached before the next, longer chain.
        let mut definitions = serde_json::Map::new();
        definitions.insert("n00".into(), json!({"type": "string"}));
        for index in 1..MAX_DEPTH + 2 {
            definitions.insert(
                format!("n{index:02}"),
                json!({
                    "$ref": format!("#/$defs/n{:02}", index - 1)
                }),
            );
        }
        let error = validate_tool_schema(&json!({"$defs": definitions})).unwrap_err();
        assert!(error.contains("depth limit"), "{error}");
    }
}
