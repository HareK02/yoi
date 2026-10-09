//! Input-only compact Interface addresses. Never parse rendered signatures.
use super::{InterfaceReference, ToolError};

#[derive(Debug, PartialEq, Eq)]
pub(super) enum Target {
    Object(String),
    Interface(InterfaceReference),
}

fn invalid() -> ToolError {
    ToolError::InvalidArgument(
        "Inspect requires an absolute Object path or scope::name Interface address; JSON-quote special components, and quote an entire Object path containing ::. Operation suffixes are not Inspect targets.".into(),
    )
}

fn quoted(input: &str) -> Result<(String, &str), ToolError> {
    let mut stream = serde_json::Deserializer::from_str(input).into_iter::<String>();
    let value = stream.next().ok_or_else(invalid)?.map_err(|_| invalid())?;
    Ok((value, &input[stream.byte_offset()..]))
}

fn bare(value: &str, scope: bool) -> bool {
    !value.is_empty()
        && value
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b"_.-".contains(&b) || (scope && b == b'/'))
}

pub(super) fn parse(input: &str) -> Result<Target, ToolError> {
    let (scope, remainder) = if input.starts_with('"') {
        quoted(input)?
    } else if let Some(boundary) = input.find("::") {
        let (scope, remainder) = input.split_at(boundary);
        if !bare(scope, true) {
            return Err(invalid());
        }
        (scope.to_owned(), remainder)
    } else {
        // Ordinary Object paths are Protocol paths, not restricted Text View tokens.
        wip_protocol::validate_path(input).map_err(|_| invalid())?;
        return Ok(Target::Object(input.to_owned()));
    };
    wip_protocol::validate_path(&scope).map_err(|_| invalid())?;
    if remainder.is_empty() {
        return Ok(Target::Object(scope));
    }
    let name = remainder.strip_prefix("::").ok_or_else(invalid)?;
    let name = if name.starts_with('"') {
        let (value, rest) = quoted(name)?;
        if !rest.is_empty() {
            return Err(invalid());
        }
        value
    } else {
        if !bare(name, false) {
            return Err(invalid());
        }
        name.to_owned()
    };
    let reference = InterfaceReference { scope, name };
    reference.validate().map_err(|_| invalid())?;
    Ok(Target::Interface(reference))
}

pub(super) fn interface_path(reference: &InterfaceReference) -> String {
    fn component(value: &str, scope: bool) -> String {
        if bare(value, scope) {
            value.to_owned()
        } else {
            serde_json::to_string(value).expect("string serializes")
        }
    }
    format!(
        "{}::{}",
        component(&reference.scope, true),
        component(&reference.name, false)
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn compact_addresses_roundtrip_exact_components_without_normalization() {
        for scope in ["/", "/github", "/github::archive", "/開発", "/x\"\n"] {
            for name in [
                "issue.read",
                "same::name",
                "yoi.tool/Read/v1",
                "課題#read",
                "x\"\\\n",
            ] {
                let reference = InterfaceReference {
                    scope: scope.into(),
                    name: name.into(),
                };
                assert_eq!(
                    parse(&interface_path(&reference)).unwrap(),
                    Target::Interface(reference)
                );
            }
        }
        assert_eq!(
            parse("\"/github\"::\"issue.read\"").unwrap(),
            Target::Interface(InterfaceReference {
                scope: "/github".into(),
                name: "issue.read".into()
            })
        );
    }

    #[test]
    fn object_paths_are_not_interfaces_and_quoted_delimiters_remain_literal() {
        for path in ["/", "/tools/Read", "/開発", "/x\"\n", "/item#read"] {
            assert_eq!(parse(path).unwrap(), Target::Object(path.into()));
            assert_eq!(
                parse(&serde_json::to_string(path).unwrap()).unwrap(),
                Target::Object(path.into())
            );
        }
        assert_eq!(
            parse("\"/literal::name\"").unwrap(),
            Target::Object("/literal::name".into())
        );
    }

    #[test]
    fn invalid_or_ambiguous_addresses_do_not_infer_scope_or_operation() {
        for input in [
            "",
            "relative",
            "relative::name",
            "::name",
            "/::",
            "/::\"\"",
            "/::name#read",
            "/::name::other",
            "/::unquoted/name",
            "/::\"name\"#read",
            "\"/\"junk",
            "\"/unterminated",
            "/::\"bad\\q\"",
            "/bad/../scope::name",
            "/:: name",
            "/::\"name\" ",
        ] {
            assert!(parse(input).is_err(), "accepted {input:?}");
        }
    }
}
