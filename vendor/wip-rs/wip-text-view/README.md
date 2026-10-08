# wip-text-view

Standalone Object and Interface signature rendering for WIP Text View, following
`4.2-text-view.md` at reference revision `6086f3c` and the Protocol semantics in
`3-protocol.md`. Only `wip-protocol` and the Rust standard library are dependencies.
No Client, Host, HTTP I/O, cache, UI framework or retrieval service is required.

## Single-entity API

```rust
use wip_protocol::{Object, InterfaceReference, InterfaceDescriptor, INTERFACE_FORMAT_V1};
use wip_text_view::{Interface, render_object, render_interface};

let reference = InterfaceReference { scope: "/".into(), name: "example".into() };
let object = Object {
    name: "item".into(), description: None, interfaces: vec![reference.clone()],
    r#ref: None, validator: None,
};
let object_signature = render_object(&object)?;
assert_eq!(object_signature,
    "object {\n  name: \"item\";\n  interfaces: [\"/::example\"];\n}\n");

let descriptor = InterfaceDescriptor {
    format: INTERFACE_FORMAT_V1.into(), documentation: None,
    types: vec![], operations: vec![],
};
let interface = Interface { reference: &reference, descriptor: &descriptor };
let interface_signature = render_interface(&interface)?;
assert_eq!(interface_signature, "interface \"/::example\" {\n}\n");
# Ok::<(), wip_text_view::RenderError>(())
```

`Interface<'a>` is one borrowed SDK input entity containing a complete structured
reference and its corresponding descriptor, not a wire schema extension. The
caller pairs them from a coherent observation or publication. The descriptor has
no identity, so a renderer cannot prove the pairing's provenance or freshness.
For example, a caller with a validated `FetchInterfaceResponse` can borrow its
`interface` and `descriptor`; it does not pass `scope_ref` or validators.

An Object has no placement path. This API always emits `object { ... }`, never
requests a path and never infers one from name or ref. It requires no descriptor.
An Interface needs no Object, separate name/scope/context arguments or lookup.
There is no document aggregator, configuration or details selector.

## Typed display tokens

```rust
use wip_protocol::{Object, InterfaceReference, InterfaceDescriptor, INTERFACE_FORMAT_V1};
use wip_text_view::{
    Interface, TokenKind, tokenize_object, tokenize_interface, to_plain_text,
    render_object, render_interface,
};

let reference = InterfaceReference { scope: "/".into(), name: "example".into() };
let object = Object {
    name: "item".into(), description: None, interfaces: vec![reference.clone()],
    r#ref: None, validator: None,
};
let object_tokens = tokenize_object(&object)?;
assert_eq!(object_tokens[0].kind(), TokenKind::Keyword);
assert_eq!(object_tokens[0].text(), "object");
assert_eq!(to_plain_text(&object_tokens), render_object(&object)?);

let descriptor = InterfaceDescriptor {
    format: INTERFACE_FORMAT_V1.into(), documentation: None,
    types: vec![], operations: vec![],
};
let input = Interface { reference: &reference, descriptor: &descriptor };
let tokens = tokenize_interface(&input)?;
for token in &tokens {
    // Custom renderers can select presentation by role without reparsing text.
    if token.kind() == TokenKind::Reference {
        assert_eq!(token.text(), "\"/::example\"");
    }
}
let joined: String = tokens.iter().map(|token| token.text()).collect();
assert_eq!(to_plain_text(&tokens), joined);
assert_eq!(to_plain_text(&tokens), render_interface(&input)?);
# Ok::<(), wip_text_view::RenderError>(())
```

`tokenize_object` / `tokenize_interface` return `Result<Vec<Token>, RenderError>`.
They generate **directly from the input structure**, not by lexing a completed
Text View string. Each token owns its text (and can outlive the input) and exposes
`kind()` / `text()`. Existing render APIs use the same token generation followed
by `to_plain_text(&[Token])`; there is no second grammar/escape/layout engine.

### Kind and splitting contract

| Kind | One token contains |
| --- | --- |
| `Keyword` | One grammar keyword, Object property label, or primitive type keyword |
| `TypeName` | One type declaration name or Named type reference |
| `OperationName` | One Operation declaration name |
| `ParameterName` / `FieldName` | One parameter / record field name |
| `CaseName` | One enum or union case name |
| `String` | One complete quoted Object name or description |
| `Reference` | One complete quoted absolute Interface reference, with both quoting layers |
| `Documentation` | Entire `/// "summary"` content, including marker and internal space, excluding indentation/LF |
| `Symbol` | One punctuation mark, except the indivisible two-character `->` arrow |
| `Whitespace` | One inter-token ASCII space |
| `Newline` | One LF, including the final LF |
| `Indentation` | All leading spaces of a non-root line, two spaces per depth level |

Quoted identifiers retain their semantic role even when named like keywords.
Quotes, backticks and all WIP escapes are part of the data token, never separate
symbols. Host spaces inside quoted data remain in that token. Adjacent symbols
are separate; no empty tokens or zero-depth indentation tokens are emitted.
`TokenKind` is non-exhaustive so consumers should handle future roles with a
fallback. The API does not distinguish type declaration names from Named uses,
or enum cases from union cases; both share the documented role.

All layout is included. Concatenate token text **without separators**, not with
`join(" ")`. `to_plain_text` is only that concatenation: no interpretation,
validation, quoting, escaping or layout completion. An empty slice yields `""`;
a filtered/reordered slice is allowed but need not be a complete signature.

Token text is escaped for **WIP Text View**, not for HTML or other destinations.
Custom renderers must escape for their destination and treat Host text as data,
not markup or instructions. This crate supplies no HTML, ANSI, color or theme
renderer/adapter and no external text lexer/parser or AST.

## Output and validation

- UTF-8 plain text with LF, two-space indentation and a final LF, without ANSI,
  Markdown fences, syntax highlighting or provider envelopes. Nonempty composites
  and parameter lists use multiple lines; layout is deterministic here, but not a
  byte-canonical protocol format.
- All Object references remain in Host order. All types, then all operations,
  fields, parameters and cases remain in declaration order, without elision.
- Missing vs empty description/documentation, optional vs unit, no payload vs
  unit payload, and empty record/enum/union/declaration sets remain distinct.
- Full absolute references use component quoting **and** outer JSON-string quoting.
  Identifiers use keywords/backtick rules. All Host strings use standard JSON
  escapes plus DEL/C1, Unicode line/paragraph separator and bidi-control escapes.
  No normalization, case conversion, URL encoding or input mutation occurs.
- Documentation summaries are displayed in their prescribed positions, including
  return documentation between `)` and `->`. Details are not displayed. Host text
  is data, not renderer instructions or markup; quoting alone is not a prompt
  injection defense. AI adapters must preserve the trust boundary.
- Object ref/validator, children, domain data and Interface scope_ref/validator
  are not signature content. No annotations are inferred from operation names.

The render functions return `Result<String, RenderError>`; token generation
returns `Result<Vec<Token>, RenderError>`. Neither returns partial signatures. Resource checks happen first, followed by existing Protocol validators:
Object shape and reference uniqueness, or Interface reference shape plus descriptor
structure, duplicate names, unknown Named references and cycles. Descriptor format
is checked after semantics: valid but unsupported formats return
`UnsupportedDescriptorFormat`, distinct from `InvalidDescriptor`. Supported format
is exactly `wip-interface/1`. Other validation errors preserve the Protocol's typed
`ValidationError` (also exposed as the error source).

Bounded input limits are **1 MiB** of total string/opaque byte data, **16,384**
declarations/fields/parameters/cases/type expressions/references, **256** named type
declarations, and **64** TypeExpr nesting levels. Details and Object runtime
metadata count toward input bytes even though they are not rendered. A limit
returns `RenderError::ResourceLimit`, never truncates or supplies `...`. Counts
bound allocation and recursive validation before those validators run; these are
library safety limits, not Protocol restrictions.

Without path/context, this library cannot attest Object scope ancestry, placement,
observation consistency, identity, membership, authorization or currentness.
Callers validate those at their observation boundary. Callers also own retrieval,
resolution, cache, selection, multiple-signature arrangement and AI/UI adaptation.
Text is display-only, not serialization, a digest, identity/cache key or call
target. No parser or round-trip API is supplied.

## Validation

`tests/signatures.rs` is an external consumer of the public APIs. Goldens cover
the spec's Object, issue.read, CloseResult, search/return documentation and
identifier examples (with permitted multiline layout), plus nested documentation.
The suite checks all TypeExpr variants/positions, ordering and empty distinctions,
keyword and Unicode names, both reference quoting layers, all required control
escapes, input immutability, metadata exclusion and typed rejection cases. Every
existing case compares the render result (including exact errors) with token
text concatenation and `to_plain_text`. Additional tests assert complete Object
token splitting, all semantic roles, keyword collisions, atomic escaped data,
all documentation positions, layout tokens and ownership beyond input lifetime.
Input budgets bound token counts per node, indentation depth and escape expansion;
token generation checks them before any rendering/validation traversal.

```console
cargo test -p wip-text-view
cargo clippy -p wip-text-view --all-targets -- -D warnings
RUSTDOCFLAGS='-D warnings' cargo doc -p wip-text-view --no-deps
```
