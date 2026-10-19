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

use crate::wip::{
    WipCallContext, WipOperationError, WipOperationHandler, WipOperationOutput, WipProjection,
};
use async_trait::async_trait;
use sha2::{Digest, Sha256};
use std::{collections::BTreeMap, sync::Arc};
use wip_protocol::{
    Documentation, FieldDeclaration, INTERFACE_FORMAT_V1, InterfaceDescriptor, InterfaceReference,
    OperationDeclaration, ProtocolError, ProtocolErrorCode, ReturnDeclaration, Value,
};

pub(crate) const MAX_LINES: usize = 2000;
pub(crate) const MAX_BYTES: usize = 64 * 1024;

/// Contracts have independent capability membership and no target-dependent descriptor fields.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Contract {
    ReadLines,
    SearchText,
    Edit,
    Write,
    List,
    Glob,
    Grep,
    CreateFile,
}
impl Contract {
    pub(crate) fn reference(self) -> InterfaceReference {
        crate::wip::root_reference(match self {
            Self::ReadLines => "yoi.file/read-lines/v1",
            Self::SearchText => "yoi.file/search-text/v1",
            Self::Edit => "yoi.file/edit-text/v1",
            Self::Write => "yoi.file/write-text/v1",
            Self::List => "yoi.directory/list/v1",
            Self::Glob => "yoi.directory/glob/v1",
            Self::Grep => "yoi.directory/grep/v1",
            Self::CreateFile => "yoi.directory/create-file/v1",
        })
    }
    pub(crate) fn from_reference(r: &InterfaceReference) -> Option<Self> {
        [
            Self::ReadLines,
            Self::SearchText,
            Self::Edit,
            Self::Write,
            Self::List,
            Self::Glob,
            Self::Grep,
            Self::CreateFile,
        ]
        .into_iter()
        .find(|c| c.reference() == *r)
    }
    pub(crate) fn operation(self) -> &'static str {
        match self {
            Self::ReadLines => "read",
            Self::SearchText => "search",
            Self::Edit => "edit",
            Self::Write => "write",
            Self::List => "list",
            Self::Glob => "glob",
            Self::Grep => "grep",
            Self::CreateFile => "create_file",
        }
    }
    pub(crate) fn handler_operation(self) -> &'static str {
        match self {
            Self::ReadLines => "read_lines",
            Self::SearchText => "search_text",
            _ => self.operation(),
        }
    }
    pub(crate) fn descriptor(self) -> InterfaceDescriptor {
        let (summary, parameters, result) = match self {
            Self::ReadLines => (
                "Read a bounded text line window",
                parameters("read", true).unwrap(),
                page_type(false),
            ),
            Self::SearchText => {
                let mut p = parameters("read", true).unwrap();
                p.push(parameter("pattern", true, TypeExpr::String));
                (
                    "Search a bounded text line window for a literal",
                    p,
                    page_type(true),
                )
            }
            Self::Edit | Self::Write => (
                "Change existing text under provider preconditions",
                parameters(self.operation(), true).unwrap(),
                mutation_type(),
            ),
            Self::List => (
                "List direct contents, directories first then lexical path. Bounded live pages, not a snapshot; limit defaults to 100, maximum 1000; after continues by kind/entry. Includes hidden/ignored names under provider List policy, unlike Glob/Grep.",
                vec![
                    parameter("limit", false, TypeExpr::Integer),
                    parameter("after", false, cursor_type()),
                ],
                result_type(true),
            ),
            Self::Glob => (
                "Glob within this directory",
                vec![
                    parameter("pattern", true, TypeExpr::String),
                    parameter("path", false, TypeExpr::String),
                ],
                result_type(false),
            ),
            Self::Grep => (
                "Regex search within this directory",
                grep_parameters(),
                result_type(false),
            ),
            Self::CreateFile => (
                "Create a new relative file and permitted missing parents; never overwrite",
                vec![
                    parameter("path", true, TypeExpr::String),
                    parameter("content", true, TypeExpr::String),
                ],
                mutation_type(),
            ),
        };
        InterfaceDescriptor {format:INTERFACE_FORMAT_V1.into(),types:vec![],documentation:Some(Documentation {summary:summary.into(),details:Some(match self {
            Self::ReadLines|Self::SearchText=>"UTF-8 text (invalid source bytes decoded lossily); Rust lines semantics (CRLF stripped, final newline is not an extra line). Zero-based offset defaults to 0. Limit defaults to 2000, must be 1..=2000. Source/result windows exceeding 64 KiB are refused, never silently skipped. Read returns one-based numbered content. Search is case-sensitive literal matching, pattern nonempty and at most 4096 bytes, returning one-based line/text records. next_offset advances by scanned lines including nonmatches; truncated means more lines remain. Provider source bounds may refuse retrieval. InvalidArguments rejects bad ranges/patterns; ResourceLimitExceeded rejects bounds; ValidatorMismatch rejects stale state; PermissionDenied rejects current authority. No automatic retry.",
            Self::Edit|Self::Write|Self::CreateFile=>"UTF-8 text with provider size bounds. Checked save, prior Read and CAS requirements remain provider-owned. Edit rejects empty or identical old/new strings, missing or ambiguous matches unless replace_all. Success returns metadata with provider commit information. InvalidArguments rejects invalid edits; ResourceLimitExceeded rejects bounds; ValidatorMismatch rejects stale state; PermissionDenied rejects current authority. Post-dispatch failure may be OutcomeUnknown: reconcile, never automatically retry.",
            _=>"Paths and cursors are Worldspace entries, not host paths. Provider scope, permission, visibility, bounds and cancellation remain authoritative. Discovery does not acquire every result or add indexable children. InvalidArguments, ResourceLimitExceeded, PermissionDenied and ValidatorMismatch preserve provider failures. No automatic retry.",
        }.into())}),operations:vec![OperationDeclaration {name:self.operation().into(),documentation:None,parameters,returns:ReturnDeclaration {documentation:None,r#type:result}}]}
    }
    pub(crate) fn validator(self) -> Vec<u8> {
        Sha256::digest(format!("{:?}", self.descriptor())).to_vec()
    }
}
pub(crate) fn expose(p: &mut WipProjection, contracts: impl IntoIterator<Item = Contract>) {
    for c in contracts {
        let r = c.reference();
        if !p.object.interfaces.contains(&r) {
            p.object.interfaces.push(r);
        }
    }
}

/// Select only from current provider membership; the selected handler still owns all authority.
pub(crate) fn select(p: &mut WipProjection, r: &InterfaceReference) -> bool {
    let Some(c) = Contract::from_reference(r) else {
        return false;
    };
    if !p.object.interfaces.contains(r) {
        return false;
    }
    p.interface = r.clone();
    p.descriptor = c.descriptor();
    p.interface_validator = Some(c.validator());
    p.handler = Arc::new(Selected {
        contract: c,
        inner: p.handler.clone(),
    });
    true
}
struct Selected {
    contract: Contract,
    inner: Arc<dyn WipOperationHandler>,
}
#[async_trait]
impl WipOperationHandler for Selected {
    async fn call(
        &self,
        _: &str,
        args: &BTreeMap<String, Value>,
        context: WipCallContext,
    ) -> Result<WipOperationOutput, WipOperationError> {
        let mut out = self
            .inner
            .call(self.contract.handler_operation(), args, context)
            .await?;
        if matches!(
            self.contract,
            Contract::Edit | Contract::Write | Contract::CreateFile
        ) {
            out.value = Value::Record(BTreeMap::from([("metadata".into(), out.value)]));
        }
        Ok(out)
    }
    async fn cancel_operation(
        &self,
        _: &str,
        ctx: &agen::tool::ToolExecutionContext,
    ) -> Result<(), agen::tool::ToolError> {
        self.inner
            .cancel_operation(self.contract.handler_operation(), ctx)
            .await
    }
}
fn failure(code: ProtocolErrorCode, message: &str) -> WipOperationError {
    WipOperationError::Protocol(ProtocolError {
        code,
        message: message.into(),
    })
}
pub(crate) fn window(
    args: &BTreeMap<String, Value>,
    search: bool,
) -> Result<(usize, usize), WipOperationError> {
    if args
        .keys()
        .any(|k| !["offset", "limit"].contains(&k.as_str()) && !(search && k == "pattern"))
    {
        return Err(failure(
            ProtocolErrorCode::InvalidArguments,
            "unknown text argument",
        ));
    }
    let integer = |name: &str, default: usize| match args.get(name) {
        None => Ok(default),
        Some(Value::Integer(i)) => usize::try_from(*i)
            .map_err(|_| failure(ProtocolErrorCode::InvalidArguments, "negative text range")),
        _ => Err(failure(
            ProtocolErrorCode::InvalidArguments,
            "invalid text range",
        )),
    };
    let offset = integer("offset", 0)?;
    let limit = integer("limit", MAX_LINES)?;
    if !(1..=MAX_LINES).contains(&limit) || offset > i64::MAX as usize - MAX_LINES {
        return Err(failure(
            ProtocolErrorCode::InvalidArguments,
            "text range exceeds bounds",
        ));
    }
    if search {
        match args.get("pattern") {
            Some(Value::String(p)) if !p.is_empty() && p.len() <= 4096 => {}
            _ => {
                return Err(failure(
                    ProtocolErrorCode::InvalidArguments,
                    "search requires a nonempty literal pattern up to 4096 bytes",
                ));
            }
        }
    }
    Ok((offset, limit))
}
pub(crate) fn text_result(
    op: &str,
    args: &BTreeMap<String, Value>,
    content: &str,
) -> Result<Value, WipOperationError> {
    let (offset, limit) = window(args, op == "search_text")?;
    let mut lines = content.lines().skip(offset);
    let mut text = String::new();
    for line in lines.by_ref().take(limit) {
        if text.len().saturating_add(line.len()).saturating_add(1) >= MAX_BYTES {
            return Err(failure(
                ProtocolErrorCode::ResourceLimitExceeded,
                "text window exceeds source byte bound",
            ));
        }
        text.push_str(line);
        text.push('\n');
    }
    page_result(op, args, &text, offset, lines.next().is_some())
}
/// A checked provider can supply only the selected window, without copying whole source files.
pub(crate) fn page_result(
    op: &str,
    args: &BTreeMap<String, Value>,
    text: &str,
    offset: usize,
    more: bool,
) -> Result<Value, WipOperationError> {
    let (_, limit) = window(args, op == "search_text")?;
    if text.len() >= MAX_BYTES {
        return Err(failure(
            ProtocolErrorCode::ResourceLimitExceeded,
            "text window exceeds source byte bound",
        ));
    }
    let count = text.lines().count();
    if count > limit {
        return Err(failure(
            ProtocolErrorCode::Internal,
            "provider returned an invalid line window",
        ));
    }
    let mut fields = BTreeMap::from([
        (
            "next_offset".into(),
            Value::Integer((offset + count) as i64),
        ),
        ("truncated".into(), Value::Boolean(more)),
    ]);
    if op == "search_text" {
        let Some(Value::String(pattern)) = args.get("pattern") else {
            unreachable!("window checked pattern")
        };
        let matches = text
            .lines()
            .enumerate()
            .filter(|(_, l)| l.contains(pattern))
            .map(|(i, l)| {
                Value::Record(BTreeMap::from([
                    ("line".into(), Value::Integer((offset + i + 1) as i64)),
                    ("text".into(), Value::String(l.into())),
                ]))
            })
            .collect();
        fields.insert("matches".into(), Value::List(matches));
    } else {
        let full_len = text
            .lines()
            .enumerate()
            .map(|(i, l)| format!("{:>6}\t", offset + i + 1).len() + l.len() + 1)
            .sum::<usize>();
        if full_len > MAX_BYTES.saturating_sub(64) {
            return Err(failure(
                ProtocolErrorCode::ResourceLimitExceeded,
                "numbered window exceeds output byte bound",
            ));
        }
        let rendered =
            fs_operation::text::render_numbered(text, offset, offset + count, false, MAX_BYTES);
        fields.insert("content".into(), Value::String(rendered.body));
    }
    Ok(Value::Record(fields))
}
fn parameter(n: &str, r: bool, t: TypeExpr) -> ParameterDeclaration {
    ParameterDeclaration {
        name: n.into(),
        required: r,
        documentation: None,
        r#type: t,
    }
}
fn field(n: &str, r: bool, t: TypeExpr) -> FieldDeclaration {
    FieldDeclaration {
        name: n.into(),
        required: r,
        documentation: None,
        r#type: t,
    }
}
fn mutation_type() -> TypeExpr {
    TypeExpr::Record {
        fields: vec![field("metadata", true, TypeExpr::Json)],
    }
}
fn page_type(search: bool) -> TypeExpr {
    let mut fields = vec![
        field("next_offset", true, TypeExpr::Integer),
        field("truncated", true, TypeExpr::Boolean),
    ];
    fields.push(if search {
        field(
            "matches",
            true,
            TypeExpr::List {
                items: Box::new(TypeExpr::Record {
                    fields: vec![
                        field("line", true, TypeExpr::Integer),
                        field("text", true, TypeExpr::String),
                    ],
                }),
            },
        )
    } else {
        field("content", true, TypeExpr::String)
    });
    TypeExpr::Record { fields }
}
fn cursor_type() -> TypeExpr {
    TypeExpr::Record {
        fields: vec![
            field("kind", true, TypeExpr::String),
            field("entry", true, TypeExpr::Entry),
        ],
    }
}
fn result_type(list: bool) -> TypeExpr {
    let mut item = vec![field("entry", true, TypeExpr::Entry)];
    if list {
        item.extend([
            field("kind", true, TypeExpr::String),
            field("size", true, TypeExpr::Integer),
        ]);
    }
    let mut fields = vec![
        field("summary", true, TypeExpr::String),
        field("content", false, TypeExpr::String),
        field(
            "items",
            true,
            TypeExpr::List {
                items: Box::new(TypeExpr::Record { fields: item }),
            },
        ),
    ];
    if list {
        fields.extend([
            field("truncated", true, TypeExpr::Boolean),
            field("after", false, cursor_type()),
        ]);
    }
    TypeExpr::Record { fields }
}
fn grep_parameters() -> Vec<ParameterDeclaration> {
    let mut p = vec![parameter("pattern", true, TypeExpr::String)];
    for n in ["path", "glob", "type", "output_mode"] {
        p.push(parameter(n, false, TypeExpr::String));
    }
    for n in ["-A", "-B", "-C", "head_limit", "offset"] {
        p.push(parameter(n, false, TypeExpr::Integer));
    }
    for n in ["case_insensitive", "multiline"] {
        p.push(parameter(n, false, TypeExpr::Boolean));
    }
    p
}
