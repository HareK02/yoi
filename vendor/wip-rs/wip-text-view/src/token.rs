//! Owned display data, independent of the Protocol wire model.

/// Semantic display role assigned from the input structure, not from text parsing.
///
/// Identifiers retain their role even when quoted or identical to a keyword.
/// Type declarations and Named expressions both use [`Self::TypeName`]. Enum and
/// union cases both use [`Self::CaseName`]. See [`Token`] for splitting rules.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
#[non_exhaustive]
pub enum TokenKind {
    /// Grammar keyword, including Object property labels and primitive types.
    Keyword,
    /// Named type declaration or reference within the same Interface.
    TypeName,
    /// Operation declaration name (not an invocation target).
    OperationName,
    /// Operation parameter name.
    ParameterName,
    /// Record field name.
    FieldName,
    /// Enum or union case name.
    CaseName,
    /// Quoted Object name or description.
    String,
    /// Entire quoted absolute Interface reference, including both quoting layers.
    Reference,
    /// Entire `/// "summary"` line content, excluding indentation and LF.
    Documentation,
    /// One punctuation mark, or the complete two-character return arrow `->`.
    Symbol,
    /// One inter-token ASCII space, never indentation or text inside quotes.
    Whitespace,
    /// One LF (`\n`), including the final signature LF.
    Newline,
    /// All leading spaces on a non-root line: two ASCII spaces per depth level.
    Indentation,
}

/// One owned, already escaped fragment of a Text View signature.
///
/// Each keyword, identifier, quoted string and complete quoted reference is one
/// token, including its delimiters and escapes. Documentation is one token for
/// the `///` marker, space and quoted summary together. Symbols are individual
/// punctuation marks except `->`, which is indivisible. Spaces between tokens,
/// each LF, and each nonzero line indentation have separate tokens. No empty
/// tokens are emitted; an empty value still has its quotes. Adjacent symbols are
/// not merged. Host whitespace inside quoted data stays inside that data token.
///
/// The generation APIs emit all layout: concatenating [`Self::text`] in order,
/// without separators, produces exactly the existing plain-text signature.
/// Tokens own their text and can outlive the input; they are display data, not
/// an AST, lexer output, Protocol serialization or identity/call-target data.
///
/// Text is escaped for **WIP Text View**, not for HTML or any other destination.
/// Custom renderers must apply destination-specific escaping and must not treat
/// Host documentation as markup or instructions.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Token {
    pub(crate) kind: TokenKind,
    pub(crate) text: String,
}

impl Token {
    /// Returns the semantic display role, without reparsing the output text.
    pub fn kind(&self) -> TokenKind {
        self.kind
    }

    /// Returns the exact output fragment, with WIP quoting and escapes applied.
    pub fn text(&self) -> &str {
        &self.text
    }
}

/// Concatenates token text in order, with no separators or further processing.
///
/// This performs no grammar interpretation, validation, quoting, escaping or
/// layout completion. An empty slice yields an empty string. Filtering or
/// reordering tokens is allowed, but need not produce a complete signature.
/// Object/Interface generation limits are checked by [`crate::tokenize_object`]
/// and [`crate::tokenize_interface`], not by this concatenation function.
pub fn to_plain_text(tokens: &[Token]) -> String {
    tokens.iter().map(Token::text).collect()
}
