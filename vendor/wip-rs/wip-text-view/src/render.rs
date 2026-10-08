use std::fmt::Write as _;
use wip_protocol::{Documentation, InterfaceReference, Object, OperationDeclaration, TypeExpr};

use crate::{Interface, Token, TokenKind};
use TokenKind::{
    CaseName, FieldName, Indentation, Keyword, Newline, OperationName, ParameterName, Reference,
    Symbol, TypeName, Whitespace,
};

// Quoting a component and quoting the complete reference are separate layers.
fn reference(reference: &InterfaceReference) -> String {
    format!(
        "{}::{}",
        component(&reference.scope, true),
        component(&reference.name, false)
    )
}

fn component(value: &str, scope: bool) -> String {
    if !value.is_empty()
        && value
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b"_.-".contains(&b) || (scope && b == b'/'))
    {
        value.into()
    } else {
        quote(value, '"')
    }
}

fn identifier(value: &str) -> String {
    const KEYWORDS: &[&str] = &[
        "object",
        "at",
        "name",
        "description",
        "interfaces",
        "interface",
        "type",
        "operation",
        "enum",
        "union",
        "unit",
        "boolean",
        "integer",
        "number",
        "string",
        "bytes",
        "json",
        "entry",
    ];
    if value
        .bytes()
        .next()
        .is_some_and(|b| b.is_ascii_alphabetic() || b == b'_')
        && value
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b == b'_')
        && !KEYWORDS.contains(&value)
    {
        value.into()
    } else {
        quote(value, '`')
    }
}

fn quote(value: &str, delimiter: char) -> String {
    let mut output = String::new();
    output.push(delimiter);
    for ch in value.chars() {
        match ch {
            '`' if delimiter == '`' => output.push_str("\\`"),
            '\\' => output.push_str("\\\\"),
            '"' => output.push_str("\\\""),
            '\u{08}' => output.push_str("\\b"),
            '\u{0c}' => output.push_str("\\f"),
            '\n' => output.push_str("\\n"),
            '\r' => output.push_str("\\r"),
            '\t' => output.push_str("\\t"),
            '\u{00}'..='\u{1f}'
            | '\u{7f}'..='\u{9f}'
            | '\u{2028}'
            | '\u{2029}'
            | '\u{061c}'
            | '\u{200e}'..='\u{200f}'
            | '\u{202a}'..='\u{202e}'
            | '\u{2066}'..='\u{2069}' => {
                write!(output, "\\u{:04x}", ch as u32).expect("String write");
            }
            _ => output.push(ch),
        }
    }
    output.push(delimiter);
    output
}

pub(crate) fn object(object: &Object) -> Vec<Token> {
    let mut writer = Writer(Vec::new());
    writer.word(Keyword, "object");
    writer.space();
    writer.symbol("{");
    writer.newline();
    writer.indent(1);
    writer.word(Keyword, "name");
    writer.symbol(":");
    writer.space();
    writer.word(TokenKind::String, quote(&object.name, '"'));
    writer.symbol(";");
    writer.newline();
    if let Some(description) = &object.description {
        writer.indent(1);
        writer.word(Keyword, "description");
        writer.symbol(":");
        writer.space();
        writer.word(TokenKind::String, quote(description, '"'));
        writer.symbol(";");
        writer.newline();
    }
    writer.indent(1);
    writer.word(Keyword, "interfaces");
    writer.symbol(":");
    writer.space();
    writer.symbol("[");
    for (index, interface) in object.interfaces.iter().enumerate() {
        if index > 0 {
            writer.symbol(",");
            writer.space();
        }
        writer.word(Reference, quote(&reference(interface), '"'));
    }
    writer.symbol("]");
    writer.symbol(";");
    writer.newline();
    writer.symbol("}");
    writer.newline();
    writer.0
}

pub(crate) fn interface(interface: &Interface<'_>) -> Vec<Token> {
    let mut writer = Writer(Vec::new());
    writer.doc(&interface.descriptor.documentation, 0);
    writer.word(Keyword, "interface");
    writer.space();
    writer.word(Reference, quote(&reference(interface.reference), '"'));
    writer.space();
    writer.symbol("{");
    writer.newline();
    for declaration in &interface.descriptor.types {
        writer.doc(&declaration.documentation, 1);
        writer.indent(1);
        writer.word(Keyword, "type");
        writer.space();
        writer.word(TypeName, identifier(&declaration.name));
        writer.space();
        writer.symbol("=");
        writer.space();
        writer.expr(&declaration.definition, 1);
        writer.symbol(";");
        writer.newline();
    }
    for operation in &interface.descriptor.operations {
        writer.operation(operation);
    }
    writer.symbol("}");
    writer.newline();
    writer.0
}

struct Writer(Vec<Token>);

impl Writer {
    fn word(&mut self, kind: TokenKind, text: impl Into<String>) {
        self.0.push(Token {
            kind,
            text: text.into(),
        });
    }

    fn symbol(&mut self, text: &str) {
        self.word(Symbol, text);
    }

    fn space(&mut self) {
        self.word(Whitespace, " ");
    }

    fn newline(&mut self) {
        self.word(Newline, "\n");
    }

    fn indent(&mut self, depth: usize) {
        if depth > 0 {
            self.word(Indentation, "  ".repeat(depth));
        }
    }

    fn doc(&mut self, doc: &Option<Documentation>, depth: usize) {
        if let Some(doc) = doc {
            self.indent(depth);
            self.word(
                TokenKind::Documentation,
                format!("/// {}", quote(&doc.summary, '"')),
            );
            self.newline();
        }
    }

    fn member(&mut self, kind: TokenKind, name: &str, required: bool, ty: &TypeExpr, depth: usize) {
        self.indent(depth);
        self.word(kind, identifier(name));
        if !required {
            self.symbol("?");
        }
        self.symbol(":");
        self.space();
        self.expr(ty, depth);
        self.symbol(",");
        self.newline();
    }

    fn operation(&mut self, operation: &OperationDeclaration) {
        self.doc(&operation.documentation, 1);
        self.indent(1);
        self.word(Keyword, "operation");
        self.space();
        self.word(OperationName, identifier(&operation.name));
        self.symbol("(");
        if !operation.parameters.is_empty() {
            self.newline();
            for parameter in &operation.parameters {
                self.doc(&parameter.documentation, 2);
                self.member(
                    ParameterName,
                    &parameter.name,
                    parameter.required,
                    &parameter.r#type,
                    2,
                );
            }
            self.indent(1);
        }
        self.symbol(")");
        if operation.returns.documentation.is_some() {
            self.newline();
            self.doc(&operation.returns.documentation, 1);
            self.indent(1);
        } else {
            self.space();
        }
        self.symbol("->");
        self.space();
        self.expr(&operation.returns.r#type, 1);
        self.symbol(";");
        self.newline();
    }

    // Starts at the current cursor. Composite closing braces align with the
    // declaration/field/parameter containing this expression, not its width.
    fn expr(&mut self, expr: &TypeExpr, depth: usize) {
        match expr {
            TypeExpr::Unit => self.word(Keyword, "unit"),
            TypeExpr::Boolean => self.word(Keyword, "boolean"),
            TypeExpr::Integer => self.word(Keyword, "integer"),
            TypeExpr::Number => self.word(Keyword, "number"),
            TypeExpr::String => self.word(Keyword, "string"),
            TypeExpr::Bytes => self.word(Keyword, "bytes"),
            TypeExpr::Json => self.word(Keyword, "json"),
            TypeExpr::Entry => self.word(Keyword, "entry"),
            TypeExpr::Named { name } => self.word(TypeName, identifier(name)),
            TypeExpr::List { items } => {
                self.symbol("[");
                self.expr(items, depth);
                self.symbol("]");
            }
            TypeExpr::Record { fields } => {
                self.symbol("{");
                if !fields.is_empty() {
                    self.newline();
                    for field in fields {
                        self.doc(&field.documentation, depth + 1);
                        self.member(
                            FieldName,
                            &field.name,
                            field.required,
                            &field.r#type,
                            depth + 1,
                        );
                    }
                    self.indent(depth);
                }
                self.symbol("}");
            }
            TypeExpr::Enum { cases } => {
                self.word(Keyword, "enum");
                self.space();
                self.symbol("{");
                if !cases.is_empty() {
                    self.newline();
                    for case in cases {
                        self.doc(&case.documentation, depth + 1);
                        self.indent(depth + 1);
                        self.word(CaseName, identifier(&case.name));
                        self.symbol(",");
                        self.newline();
                    }
                    self.indent(depth);
                }
                self.symbol("}");
            }
            TypeExpr::Union { cases } => {
                self.word(Keyword, "union");
                self.space();
                self.symbol("{");
                if !cases.is_empty() {
                    self.newline();
                    for case in cases {
                        self.doc(&case.documentation, depth + 1);
                        self.indent(depth + 1);
                        self.word(CaseName, identifier(&case.name));
                        if let Some(payload) = &case.payload {
                            self.symbol("(");
                            self.expr(payload, depth + 1);
                            self.symbol(")");
                        }
                        self.symbol(",");
                        self.newline();
                    }
                    self.indent(depth);
                }
                self.symbol("}");
            }
        }
    }
}
