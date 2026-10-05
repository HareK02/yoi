//! Declarative chat Feature invocation contract.
//!
//! The input spelling is deliberately bounded and unambiguous:
//! `/name(arg=value, "positional value")`. Parentheses terminate arguments,
//! so following prose is never consumed. Strings use JSON-style quoting and
//! escaping. Unquoted values are limited to booleans, integers, enum-like words,
//! and paths without comma/parenthesis/whitespace. This module parses input for
//! clients and validates the structured payload again at the host boundary.

use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, BTreeSet};
use std::fmt;

/// UTF-8 byte bound shared by input validation and durable recovery keys.
pub const MAX_FEATURE_INVOCATION_ID_BYTES: usize = 128;
/// Installed descriptors must fit a public completion entry without truncation.
pub const MAX_FEATURE_INVOCATION_DESCRIPTOR_BYTES: usize = 32 * 1024;

/// Source-qualified identity of one invocation contribution, for example
/// `builtin:attachments/attach`. Display names and aliases are never authority.
#[derive(Debug, Clone, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
#[cfg_attr(feature = "typescript", derive(ts_rs::TS))]
#[cfg_attr(feature = "json-schema", derive(schemars::JsonSchema))]
pub struct FeatureInvocationIdentity(pub String);

impl fmt::Display for FeatureInvocationIdentity {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(&self.0)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "typescript", derive(ts_rs::TS))]
#[cfg_attr(feature = "json-schema", derive(schemars::JsonSchema))]
#[serde(rename_all = "snake_case")]
pub enum FeatureInvocationSyntax {
    /// `/name(value, key=value)` with JSON-style quoted strings and `)` as the
    /// explicit boundary before following natural language.
    Parenthesized,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "typescript", derive(ts_rs::TS))]
#[cfg_attr(feature = "json-schema", derive(schemars::JsonSchema))]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum InvocationArgumentType {
    String,
    Integer,
    Boolean,
    Enum {
        values: Vec<String>,
    },
    /// A Worker-readable path. This does not grant filesystem authority.
    WorkerFile,
    /// A resource selected and staged by the client. It must never be resolved
    /// as a path on the Worker host.
    ClientFile,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "typescript", derive(ts_rs::TS))]
#[cfg_attr(feature = "json-schema", derive(schemars::JsonSchema))]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum InvocationCompletion {
    None,
    Static {
        values: Vec<String>,
    },
    WorkerFile,
    ClientFile,
    /// Host-owned provider selected by source-qualified id. Metadata lookup is
    /// side-effect free; implementations never cross this wire boundary.
    Provider {
        provider: String,
    },
}

impl Default for InvocationCompletion {
    fn default() -> Self {
        Self::None
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "typescript", derive(ts_rs::TS))]
#[cfg_attr(feature = "json-schema", derive(schemars::JsonSchema))]
pub struct InvocationArgumentDescriptor {
    pub name: String,
    /// Zero-based positional index. `None` means named-only. A positional
    /// argument may also be supplied by name; this makes chip editing stable.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub position: Option<u16>,
    #[serde(default)]
    pub required: bool,
    pub value_type: InvocationArgumentType,
    #[serde(default)]
    pub completion: InvocationCompletion,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "typescript", derive(ts_rs::TS))]
#[cfg_attr(feature = "json-schema", derive(schemars::JsonSchema))]
#[serde(rename_all = "snake_case")]
pub enum InvocationClientAdapter {
    /// The client stages a local file and replaces the draft invocation with an
    /// `UploadedFile` chip. Local paths are not sent to or persisted by Worker.
    Attachment,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "typescript", derive(ts_rs::TS))]
#[cfg_attr(feature = "json-schema", derive(schemars::JsonSchema))]
pub struct FeatureInvocationDescriptor {
    pub identity: FeatureInvocationIdentity,
    pub name: String,
    #[serde(default)]
    pub aliases: Vec<String>,
    pub display_name: String,
    pub description: String,
    pub syntax: FeatureInvocationSyntax,
    #[serde(default)]
    pub arguments: Vec<InvocationArgumentDescriptor>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub client_adapter: Option<InvocationClientAdapter>,
}

impl FeatureInvocationDescriptor {
    pub fn usage(&self) -> String {
        let mut usage = format!("/{}(", self.name);
        for (index, argument) in self.arguments.iter().enumerate() {
            if index > 0 {
                usage.push_str(", ");
            }
            if argument.position.is_none() {
                usage.push_str(&argument.name);
                usage.push('=');
            }
            let marker = match &argument.value_type {
                InvocationArgumentType::String => "text",
                InvocationArgumentType::Integer => "integer",
                InvocationArgumentType::Boolean => "true|false",
                InvocationArgumentType::Enum { .. } => "choice",
                InvocationArgumentType::WorkerFile => "worker-path",
                InvocationArgumentType::ClientFile => "local-file",
            };
            if argument.required {
                usage.push('<');
                usage.push_str(marker);
                usage.push('>');
            } else {
                usage.push('[');
                usage.push_str(marker);
                usage.push(']');
            }
        }
        usage.push(')');
        usage
    }

    pub fn validate(&self) -> Result<(), InvocationValidationError> {
        const MAX_PUBLIC_NAMES: usize = 16;
        const MAX_ARGUMENTS: usize = 32;
        const MAX_METADATA_BYTES: usize = 4096;
        validate_invocation_name(&self.name)?;
        if self.aliases.len() > MAX_PUBLIC_NAMES || self.arguments.len() > MAX_ARGUMENTS {
            return Err(InvocationValidationError::InvalidDescriptor(
                "invocation descriptor exceeds collection limits".into(),
            ));
        }
        if self.display_name.len() > MAX_METADATA_BYTES
            || self.description.len() > MAX_METADATA_BYTES
        {
            return Err(InvocationValidationError::InvalidDescriptor(
                "invocation metadata exceeds byte limits".into(),
            ));
        }
        if self.identity.0.trim().is_empty()
            || !self.identity.0.contains(':')
            || self.identity.0.len() > 256
        {
            return Err(InvocationValidationError::InvalidDescriptor(
                "invocation identity must be source-qualified".into(),
            ));
        }
        let mut names = BTreeSet::new();
        let mut positions = BTreeSet::new();
        for name in std::iter::once(&self.name).chain(self.aliases.iter()) {
            validate_invocation_name(name)?;
            if !names.insert(name) {
                return Err(InvocationValidationError::InvalidDescriptor(format!(
                    "duplicate invocation name or alias `{name}`"
                )));
            }
        }
        let mut argument_names = BTreeSet::new();
        for argument in &self.arguments {
            validate_argument_name(&argument.name)?;
            if !argument_names.insert(argument.name.as_str()) {
                return Err(InvocationValidationError::InvalidDescriptor(format!(
                    "duplicate argument `{}`",
                    argument.name
                )));
            }
            if let Some(position) = argument.position
                && !positions.insert(position)
            {
                return Err(InvocationValidationError::InvalidDescriptor(format!(
                    "duplicate positional index `{position}`"
                )));
            }
            if argument
                .description
                .as_ref()
                .is_some_and(|value| value.len() > MAX_METADATA_BYTES)
            {
                return Err(InvocationValidationError::InvalidDescriptor(format!(
                    "argument `{}` description exceeds byte limits",
                    argument.name
                )));
            }
            if let InvocationCompletion::Provider { provider } = &argument.completion
                && (provider.trim().is_empty() || provider.len() > 256)
            {
                return Err(InvocationValidationError::InvalidDescriptor(format!(
                    "argument `{}` has an invalid completion provider",
                    argument.name
                )));
            }
            if matches!(&argument.value_type, InvocationArgumentType::Enum { values }
                if values.len() > 256 || values.iter().any(|value| value.len() > MAX_METADATA_BYTES))
                || matches!(&argument.completion, InvocationCompletion::Static { values }
                    if values.len() > 256 || values.iter().any(|value| value.len() > MAX_METADATA_BYTES))
            {
                return Err(InvocationValidationError::InvalidDescriptor(format!(
                    "argument `{}` completion values exceed limits",
                    argument.name
                )));
            }
            if matches!(&argument.value_type, InvocationArgumentType::Enum { values } if values.is_empty())
            {
                return Err(InvocationValidationError::InvalidDescriptor(format!(
                    "enum argument `{}` has no values",
                    argument.name
                )));
            }
        }
        for (expected, actual) in positions.iter().copied().enumerate() {
            if actual as usize != expected {
                return Err(InvocationValidationError::InvalidDescriptor(
                    "positional argument indices must be contiguous from zero".into(),
                ));
            }
        }
        if serde_json::to_vec(self).map_or(true, |encoded| {
            encoded.len() > MAX_FEATURE_INVOCATION_DESCRIPTOR_BYTES
        }) {
            return Err(InvocationValidationError::InvalidDescriptor(
                "invocation descriptor exceeds the public completion byte limit".into(),
            ));
        }
        Ok(())
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "typescript", derive(ts_rs::TS))]
#[cfg_attr(feature = "json-schema", derive(schemars::JsonSchema))]
#[serde(tag = "kind", content = "value", rename_all = "snake_case")]
pub enum InvocationValue {
    String(String),
    Integer(i32),
    Boolean(bool),
}

impl InvocationValue {
    pub fn display_input(&self) -> String {
        match self {
            Self::String(value) => quote_invocation_string(value),
            Self::Integer(value) => value.to_string(),
            Self::Boolean(value) => value.to_string(),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "typescript", derive(ts_rs::TS))]
#[cfg_attr(feature = "json-schema", derive(schemars::JsonSchema))]
pub struct InvocationArgumentValue {
    pub name: String,
    pub value: InvocationValue,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "typescript", derive(ts_rs::TS))]
#[cfg_attr(feature = "json-schema", derive(schemars::JsonSchema))]
pub struct FeatureInvocation {
    /// Stable client-generated id retained across queueing and transport retry.
    pub invocation_id: String,
    pub identity: FeatureInvocationIdentity,
    /// Selected public name, retained for exact draft/history restoration. Host
    /// resolution is always by `identity`.
    pub name: String,
    pub arguments: Vec<InvocationArgumentValue>,
}

impl FeatureInvocation {
    pub fn display_input(&self) -> String {
        let mut input = format!("/{}(", self.name);
        for (index, argument) in self.arguments.iter().enumerate() {
            if index > 0 {
                input.push_str(", ");
            }
            input.push_str(&argument.name);
            input.push('=');
            input.push_str(&argument.value.display_input());
        }
        input.push(')');
        input
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "typescript", derive(ts_rs::TS))]
#[cfg_attr(feature = "json-schema", derive(schemars::JsonSchema))]
#[serde(rename_all = "snake_case")]
pub enum FeatureInvocationStatus {
    Succeeded,
    Failed,
    OutcomeUnknown,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "typescript", derive(ts_rs::TS))]
#[cfg_attr(feature = "json-schema", derive(schemars::JsonSchema))]
pub struct FeatureInvocationResult {
    pub invocation_id: String,
    pub identity: FeatureInvocationIdentity,
    pub status: FeatureInvocationStatus,
    pub message: String,
    /// Bounded model-visible context produced by the handler. Credentials and
    /// executable implementation details must not be included.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub context: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ParsedFeatureInvocation {
    pub invocation: FeatureInvocation,
    /// UTF-8 byte offset immediately after the closing parenthesis.
    pub end: usize,
}

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum InvocationValidationError {
    #[error("invalid invocation descriptor: {0}")]
    InvalidDescriptor(String),
    #[error("incomplete invocation at byte {0}")]
    Incomplete(usize),
    #[error("unknown argument `{0}`")]
    UnknownArgument(String),
    #[error("duplicate argument `{0}`")]
    DuplicateArgument(String),
    #[error("missing required argument `{0}`")]
    MissingRequired(String),
    #[error("too many positional arguments")]
    TooManyPositional,
    #[error("invalid value for `{argument}`: {message}")]
    InvalidValue { argument: String, message: String },
    #[error("invocation identity does not match descriptor")]
    IdentityMismatch,
    #[error("invocation name does not match descriptor")]
    NameMismatch,
    #[error("invalid invocation syntax at byte {offset}: {message}")]
    Syntax { offset: usize, message: String },
}

pub fn validate_feature_invocation(
    descriptor: &FeatureInvocationDescriptor,
    invocation: &FeatureInvocation,
) -> Result<(), InvocationValidationError> {
    descriptor.validate()?;
    if invocation.identity != descriptor.identity {
        return Err(InvocationValidationError::IdentityMismatch);
    }
    if invocation.name != descriptor.name && !descriptor.aliases.contains(&invocation.name) {
        return Err(InvocationValidationError::NameMismatch);
    }
    if invocation.invocation_id.trim().is_empty()
        || invocation.invocation_id.len() > MAX_FEATURE_INVOCATION_ID_BYTES
    {
        return Err(InvocationValidationError::InvalidValue {
            argument: "invocation_id".into(),
            message: "must be nonempty and at most 128 UTF-8 bytes".into(),
        });
    }
    if invocation.arguments.len() > 32 {
        return Err(InvocationValidationError::InvalidValue {
            argument: "arguments".into(),
            message: "too many arguments".into(),
        });
    }
    let mut values = BTreeMap::new();
    for argument in &invocation.arguments {
        if values
            .insert(argument.name.as_str(), &argument.value)
            .is_some()
        {
            return Err(InvocationValidationError::DuplicateArgument(
                argument.name.clone(),
            ));
        }
    }
    for name in values.keys() {
        if !descriptor
            .arguments
            .iter()
            .any(|argument| argument.name == **name)
        {
            return Err(InvocationValidationError::UnknownArgument(
                (*name).to_string(),
            ));
        }
    }
    for argument in &descriptor.arguments {
        match values.get(argument.name.as_str()) {
            Some(value) => validate_value(argument, value)?,
            None if argument.required => {
                return Err(InvocationValidationError::MissingRequired(
                    argument.name.clone(),
                ));
            }
            None => {}
        }
    }
    Ok(())
}

/// Parse one complete parenthesized invocation starting at `start`. Callers
/// decide when input becomes a typed chip; parsing alone never executes it.
pub fn parse_feature_invocation(
    input: &str,
    start: usize,
    descriptor: &FeatureInvocationDescriptor,
    invocation_id: impl Into<String>,
) -> Result<ParsedFeatureInvocation, InvocationValidationError> {
    descriptor.validate()?;
    if !input.is_char_boundary(start) || input.as_bytes().get(start) != Some(&b'/') {
        return Err(InvocationValidationError::Syntax {
            offset: start,
            message: "expected `/`".into(),
        });
    }
    let mut cursor = start + 1;
    let name_start = cursor;
    while let Some(byte) = input.as_bytes().get(cursor) {
        if byte.is_ascii_alphanumeric() || matches!(*byte, b'-' | b'_') {
            cursor += 1;
        } else {
            break;
        }
    }
    let name = &input[name_start..cursor];
    if name != descriptor.name && !descriptor.aliases.iter().any(|alias| alias == name) {
        return Err(InvocationValidationError::NameMismatch);
    }
    skip_ws(input, &mut cursor);
    if input.as_bytes().get(cursor) != Some(&b'(') {
        return Err(InvocationValidationError::Incomplete(cursor));
    }
    cursor += 1;
    let positional = descriptor
        .arguments
        .iter()
        .filter_map(|argument| argument.position.map(|position| (position, argument)))
        .collect::<BTreeMap<_, _>>();
    let mut next_position = 0u16;
    let mut parsed = Vec::<InvocationArgumentValue>::new();
    loop {
        skip_ws(input, &mut cursor);
        match input.as_bytes().get(cursor) {
            Some(b')') => {
                cursor += 1;
                break;
            }
            None => return Err(InvocationValidationError::Incomplete(cursor)),
            _ => {}
        }
        let token_start = cursor;
        let possible_name = parse_identifier(input, &mut cursor);
        let after_name = cursor;
        skip_ws(input, &mut cursor);
        let (argument, raw_value) = if !possible_name.is_empty()
            && input.as_bytes().get(cursor) == Some(&b'=')
        {
            cursor += 1;
            skip_ws(input, &mut cursor);
            let argument = descriptor
                .arguments
                .iter()
                .find(|argument| argument.name == possible_name)
                .ok_or_else(|| InvocationValidationError::UnknownArgument(possible_name.into()))?;
            (argument, parse_raw_value(input, &mut cursor)?)
        } else {
            cursor = token_start;
            let argument = positional
                .get(&next_position)
                .copied()
                .ok_or(InvocationValidationError::TooManyPositional)?;
            next_position += 1;
            (argument, parse_raw_value(input, &mut cursor)?)
        };
        if parsed.iter().any(|value| value.name == argument.name) {
            return Err(InvocationValidationError::DuplicateArgument(
                argument.name.clone(),
            ));
        }
        let value = coerce_value(argument, raw_value)?;
        parsed.push(InvocationArgumentValue {
            name: argument.name.clone(),
            value,
        });
        skip_ws(input, &mut cursor);
        match input.as_bytes().get(cursor) {
            Some(b',') => cursor += 1,
            Some(b')') => {}
            None => return Err(InvocationValidationError::Incomplete(cursor)),
            _ => {
                return Err(InvocationValidationError::Syntax {
                    offset: after_name.max(cursor),
                    message: "expected `,` or `)`".into(),
                });
            }
        }
    }
    let invocation = FeatureInvocation {
        invocation_id: invocation_id.into(),
        identity: descriptor.identity.clone(),
        name: name.to_string(),
        arguments: parsed,
    };
    validate_feature_invocation(descriptor, &invocation)?;
    Ok(ParsedFeatureInvocation {
        invocation,
        end: cursor,
    })
}

#[derive(Debug)]
enum RawValue {
    Quoted(String),
    Bare(String),
}

fn parse_raw_value(input: &str, cursor: &mut usize) -> Result<RawValue, InvocationValidationError> {
    if input.as_bytes().get(*cursor) == Some(&b'"') {
        let start = *cursor;
        *cursor += 1;
        let mut escaped = false;
        loop {
            let Some(ch) = input[*cursor..].chars().next() else {
                return Err(InvocationValidationError::Incomplete(*cursor));
            };
            *cursor += ch.len_utf8();
            if ch == '"' && !escaped {
                let value =
                    serde_json::from_str::<String>(&input[start..*cursor]).map_err(|error| {
                        InvocationValidationError::Syntax {
                            offset: start,
                            message: error.to_string(),
                        }
                    })?;
                return Ok(RawValue::Quoted(value));
            }
            escaped = ch == '\\' && !escaped;
        }
    }
    let start = *cursor;
    while let Some(ch) = input[*cursor..].chars().next() {
        if ch == ',' || ch == ')' || ch.is_whitespace() {
            break;
        }
        *cursor += ch.len_utf8();
    }
    if *cursor == start {
        return Err(InvocationValidationError::Syntax {
            offset: start,
            message: "expected argument value".into(),
        });
    }
    Ok(RawValue::Bare(input[start..*cursor].to_string()))
}

fn coerce_value(
    descriptor: &InvocationArgumentDescriptor,
    raw: RawValue,
) -> Result<InvocationValue, InvocationValidationError> {
    let text = match raw {
        RawValue::Quoted(value) | RawValue::Bare(value) => value,
    };
    let invalid = |message: &str| InvocationValidationError::InvalidValue {
        argument: descriptor.name.clone(),
        message: message.into(),
    };
    match &descriptor.value_type {
        InvocationArgumentType::String
        | InvocationArgumentType::WorkerFile
        | InvocationArgumentType::ClientFile => Ok(InvocationValue::String(text)),
        InvocationArgumentType::Integer => {
            let digits = text.strip_prefix('-').unwrap_or(&text);
            if digits.is_empty() || !digits.bytes().all(|byte| byte.is_ascii_digit()) {
                return Err(invalid("expected a decimal integer"));
            }
            text.parse::<i32>()
                .map(InvocationValue::Integer)
                .map_err(|_| invalid("expected a signed 32-bit integer"))
        }
        InvocationArgumentType::Boolean => match text.as_str() {
            "true" => Ok(InvocationValue::Boolean(true)),
            "false" => Ok(InvocationValue::Boolean(false)),
            _ => Err(invalid("expected `true` or `false`")),
        },
        InvocationArgumentType::Enum { values } => {
            if values.contains(&text) {
                Ok(InvocationValue::String(text))
            } else {
                Err(invalid("value is not in the declared enum"))
            }
        }
    }
}

fn validate_value(
    descriptor: &InvocationArgumentDescriptor,
    value: &InvocationValue,
) -> Result<(), InvocationValidationError> {
    let valid = match (&descriptor.value_type, value) {
        (
            InvocationArgumentType::String
            | InvocationArgumentType::WorkerFile
            | InvocationArgumentType::ClientFile,
            InvocationValue::String(value),
        ) => value.len() <= 16 * 1024,
        (InvocationArgumentType::Integer, InvocationValue::Integer(_)) => true,
        (InvocationArgumentType::Boolean, InvocationValue::Boolean(_)) => true,
        (InvocationArgumentType::Enum { values }, InvocationValue::String(value)) => {
            values.contains(value)
        }
        _ => false,
    };
    if valid {
        Ok(())
    } else {
        Err(InvocationValidationError::InvalidValue {
            argument: descriptor.name.clone(),
            message: "structured value does not match the declared type".into(),
        })
    }
}

fn validate_invocation_name(name: &str) -> Result<(), InvocationValidationError> {
    if name.is_empty()
        || !name
            .bytes()
            .all(|byte| byte.is_ascii_lowercase() || byte.is_ascii_digit() || byte == b'-')
        || !name.as_bytes()[0].is_ascii_lowercase()
    {
        return Err(InvocationValidationError::InvalidDescriptor(format!(
            "invalid invocation name `{name}`"
        )));
    }
    Ok(())
}

fn validate_argument_name(name: &str) -> Result<(), InvocationValidationError> {
    if name.is_empty()
        || !name
            .bytes()
            .all(|byte| byte.is_ascii_lowercase() || byte.is_ascii_digit() || byte == b'_')
        || !name.as_bytes()[0].is_ascii_lowercase()
    {
        return Err(InvocationValidationError::InvalidDescriptor(format!(
            "invalid argument name `{name}`"
        )));
    }
    Ok(())
}

fn parse_identifier<'a>(input: &'a str, cursor: &mut usize) -> &'a str {
    let start = *cursor;
    while let Some(byte) = input.as_bytes().get(*cursor) {
        if byte.is_ascii_lowercase() || byte.is_ascii_digit() || *byte == b'_' {
            *cursor += 1;
        } else {
            break;
        }
    }
    &input[start..*cursor]
}

fn skip_ws(input: &str, cursor: &mut usize) {
    while let Some(ch) = input[*cursor..].chars().next() {
        if !ch.is_whitespace() {
            break;
        }
        *cursor += ch.len_utf8();
    }
}

pub fn quote_invocation_string(value: &str) -> String {
    // Keep the spelling identical to Web's JSON.stringify, including control
    // characters and JSON unicode escapes accepted by both common parsers.
    serde_json::to_string(value).expect("serializing a string cannot fail")
}

#[cfg(test)]
mod tests {
    use super::*;

    fn descriptor() -> FeatureInvocationDescriptor {
        FeatureInvocationDescriptor {
            identity: FeatureInvocationIdentity("builtin:test/run".into()),
            name: "run".into(),
            aliases: vec!["execute".into()],
            display_name: "Run".into(),
            description: "test invocation".into(),
            syntax: FeatureInvocationSyntax::Parenthesized,
            arguments: vec![
                InvocationArgumentDescriptor {
                    name: "path".into(),
                    position: Some(0),
                    required: true,
                    value_type: InvocationArgumentType::String,
                    completion: InvocationCompletion::WorkerFile,
                    description: None,
                },
                InvocationArgumentDescriptor {
                    name: "mode".into(),
                    position: None,
                    required: false,
                    value_type: InvocationArgumentType::Enum {
                        values: vec!["fast".into(), "safe".into()],
                    },
                    completion: InvocationCompletion::Static {
                        values: vec!["fast".into(), "safe".into()],
                    },
                    description: None,
                },
            ],
            client_adapter: None,
        }
    }

    #[test]
    fn parses_unicode_quoted_paths_and_stops_before_natural_language() {
        let input = "/run(\"資料/my file / x\", mode=safe) この後は本文";
        let parsed = parse_feature_invocation(input, 0, &descriptor(), "invoke-1").unwrap();
        assert_eq!(&input[parsed.end..], " この後は本文");
        assert_eq!(
            parsed.invocation.arguments,
            vec![
                InvocationArgumentValue {
                    name: "path".into(),
                    value: InvocationValue::String("資料/my file / x".into()),
                },
                InvocationArgumentValue {
                    name: "mode".into(),
                    value: InvocationValue::String("safe".into()),
                },
            ]
        );
    }

    #[test]
    fn reports_incomplete_unknown_duplicate_missing_and_type_errors() {
        assert!(matches!(
            parse_feature_invocation("/run(\"x\"", 0, &descriptor(), "1"),
            Err(InvocationValidationError::Incomplete(_))
        ));
        assert!(matches!(
            parse_feature_invocation("/run(path=x, nope=y)", 0, &descriptor(), "1"),
            Err(InvocationValidationError::UnknownArgument(name)) if name == "nope"
        ));
        assert!(matches!(
            parse_feature_invocation("/run(path=x, path=y)", 0, &descriptor(), "1"),
            Err(InvocationValidationError::DuplicateArgument(name)) if name == "path"
        ));
        assert!(matches!(
            parse_feature_invocation("/run(mode=fast)", 0, &descriptor(), "1"),
            Err(InvocationValidationError::MissingRequired(name)) if name == "path"
        ));
        assert!(matches!(
            parse_feature_invocation("/run(x, mode=unknown)", 0, &descriptor(), "1"),
            Err(InvocationValidationError::InvalidValue { argument, .. }) if argument == "mode"
        ));
    }

    #[test]
    fn descriptors_cannot_install_with_undiscoverable_oversized_metadata() {
        let mut descriptor = descriptor();
        descriptor.arguments[0].completion = InvocationCompletion::Static {
            values: vec!["a".repeat(1024); 40],
        };
        assert!(
            matches!(descriptor.validate(), Err(InvocationValidationError::InvalidDescriptor(message)) if message.contains("completion byte limit"))
        );
    }

    #[test]
    fn client_and_recovery_invocation_key_bound_is_shared() {
        let descriptor = descriptor();
        assert!(
            parse_feature_invocation(
                "/run(x)",
                0,
                &descriptor,
                "a".repeat(MAX_FEATURE_INVOCATION_ID_BYTES)
            )
            .is_ok()
        );
        assert!(
            parse_feature_invocation(
                "/run(x)",
                0,
                &descriptor,
                "a".repeat(MAX_FEATURE_INVOCATION_ID_BYTES + 1)
            )
            .is_err()
        );
        assert!(parse_feature_invocation("/run(x)", 0, &descriptor, "界".repeat(43)).is_err());
    }

    #[test]
    fn structured_payload_validation_does_not_trust_client_identity_or_types() {
        let descriptor = descriptor();
        let original = parse_feature_invocation("/run(path=x)", 0, &descriptor, "stable")
            .unwrap()
            .invocation;
        let mut changed = original.clone();
        changed.identity = FeatureInvocationIdentity("builtin:other/run".into());
        assert_eq!(
            validate_feature_invocation(&descriptor, &changed),
            Err(InvocationValidationError::IdentityMismatch)
        );
        changed = original.clone();
        changed.name = "unregistered".into();
        assert_eq!(
            validate_feature_invocation(&descriptor, &changed),
            Err(InvocationValidationError::NameMismatch)
        );
        changed = original.clone();
        changed.arguments[0].value = InvocationValue::Boolean(true);
        assert!(matches!(
            validate_feature_invocation(&descriptor, &changed),
            Err(InvocationValidationError::InvalidValue { .. })
        ));
        changed = original.clone();
        changed.arguments.push(changed.arguments[0].clone());
        assert!(matches!(
            validate_feature_invocation(&descriptor, &changed),
            Err(InvocationValidationError::DuplicateArgument(_))
        ));
        changed = original;
        changed.invocation_id.clear();
        assert!(validate_feature_invocation(&descriptor, &changed).is_err());
    }

    #[test]
    fn integer_boolean_and_utf8_offsets_use_the_common_typed_contract() {
        let mut descriptor = descriptor();
        descriptor.arguments[0].value_type = InvocationArgumentType::Integer;
        descriptor.arguments[1].value_type = InvocationArgumentType::Boolean;
        let parsed = parse_feature_invocation(
            "本文 /execute(-2147483648, mode=true) 後",
            "本文 ".len(),
            &descriptor,
            "typed",
        )
        .unwrap();
        assert_eq!(
            parsed.invocation.arguments[0].value,
            InvocationValue::Integer(i32::MIN)
        );
        assert_eq!(
            parsed.invocation.arguments[1].value,
            InvocationValue::Boolean(true)
        );
        for invalid in ["/run(2147483648)", "/run(1.0)", "/run(1, mode=yes)"] {
            assert!(parse_feature_invocation(invalid, 0, &descriptor, "typed").is_err());
        }
        assert!(parse_feature_invocation("本文 /run(1)", 1, &descriptor, "typed").is_err());
        assert!(parse_feature_invocation("/run(1)", usize::MAX, &descriptor, "typed").is_err());
    }

    #[test]
    fn json_escapes_and_multiple_invocations_have_explicit_boundaries() {
        let input = r#"/run("\u8cc7\u6599\/a\b\f", mode=safe) prose /execute("second") end"#;
        let first = parse_feature_invocation(input, 0, &descriptor(), "first").unwrap();
        assert_eq!(
            first.invocation.arguments[0].value,
            InvocationValue::String("資料/a\u{8}\u{c}".into())
        );
        let next = input[first.end..].find("/execute").unwrap() + first.end;
        let second = parse_feature_invocation(input, next, &descriptor(), "second").unwrap();
        assert_eq!(&input[second.end..], " end");
        assert!(matches!(
            parse_feature_invocation(r#"/run("\uD800")"#, 0, &descriptor(), "bad"),
            Err(InvocationValidationError::Syntax { .. })
        ));
        let value = "controls \u{0}\u{8}\u{c}";
        let spelling = format!("/run({})", quote_invocation_string(value));
        assert_eq!(
            parse_feature_invocation(&spelling, 0, &descriptor(), "control")
                .unwrap()
                .invocation
                .arguments[0]
                .value,
            InvocationValue::String(value.into())
        );
    }

    #[test]
    fn typed_segment_round_trips_without_flattening_identity_or_arguments() {
        let parsed =
            parse_feature_invocation("/run(\"x y\", mode=fast)", 0, &descriptor(), "1").unwrap();
        let segment = crate::Segment::FeatureInvoke {
            invocation: parsed.invocation,
        };
        let json = serde_json::to_string(&segment).unwrap();
        assert!(json.contains("\"kind\":\"feature_invoke\""));
        assert!(json.contains("builtin:test/run"));
        assert_eq!(
            serde_json::from_str::<crate::Segment>(&json).unwrap(),
            segment
        );
    }

    #[test]
    fn quoting_round_trips_spaces_unicode_slashes_and_escapes() {
        let value = "a b/界\\\"\n";
        let input = format!("/run({})", quote_invocation_string(value));
        let parsed = parse_feature_invocation(&input, 0, &descriptor(), "1").unwrap();
        assert_eq!(
            parsed.invocation.arguments[0].value,
            InvocationValue::String(value.into())
        );
    }
}
