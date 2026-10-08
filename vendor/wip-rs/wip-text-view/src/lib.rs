//! Standalone, single-entity WIP Text View signatures.
//!
//! [`render_object`] accepts just an [`Object`], and [`render_interface`] accepts
//! just an [`Interface`] (a reference paired with its corresponding descriptor).
//! No Client, Host, HTTP, UI, retrieval, cache, path or display context is needed.
//! The caller supplies a coherent observation; descriptor identity cannot be
//! inferred from descriptor contents. No protocol wire fields are added.
//!
//! Output follows WIP `4.2-text-view.md` (reference revision `6086f3c`): UTF-8
//! plain text, LF, two-space indentation, all declarations in original order,
//! and documentation summaries only. There is no truncation or details setting.
//! Object paths are neither requested nor guessed: output uses `object { ... }`.
//! Runtime metadata is never rendered. Host documentation is untrusted data,
//! not markup or instructions; quoting does not by itself prevent prompt injection.
//!
//! Each call checks resource limits, then Protocol shape/semantic validation.
//! Interface rendering additionally requires [`wip_protocol::INTERFACE_FORMAT_V1`].
//! Limits reject the entire input, never produce partial text: at most 1 MiB of
//! input string/byte data, 16,384 declarations/fields/cases/type expressions or
//! references, 256 named type declarations (bounding validator graph recursion),
//! and 64 nested TypeExpr levels. These are implementation limits, not new
//! Protocol semantics. An Object without placement context can only be checked
//! for name/reference shape and duplicate references; the caller must validate
//! scope ancestry and observation consistency at its observation boundary.
//!
//! Signatures are display-only: no parser, round-trip, identity comparison,
//! cache key or call-target reconstruction is provided. The caller owns fetching,
//! freshness, authorization, selection and arrangement of multiple signatures.
//!
//! ```
//! use wip_protocol::{Object, InterfaceReference, InterfaceDescriptor, INTERFACE_FORMAT_V1};
//! use wip_text_view::{Interface, render_object, render_interface};
//! let reference = InterfaceReference { scope: "/".into(), name: "example".into() };
//! let object = Object {
//!     name: "item".into(), description: None, interfaces: vec![reference.clone()],
//!     r#ref: Some("not a path".into()), validator: Some(vec![1]),
//! };
//! assert_eq!(render_object(&object)?,
//!     "object {\n  name: \"item\";\n  interfaces: [\"/::example\"];\n}\n");
//! let descriptor = InterfaceDescriptor {
//!     format: INTERFACE_FORMAT_V1.into(), documentation: None,
//!     types: vec![], operations: vec![],
//! };
//! let interface = Interface { reference: &reference, descriptor: &descriptor };
//! assert_eq!(render_interface(&interface)?, "interface \"/::example\" {\n}\n");
//! # Ok::<(), wip_text_view::RenderError>(())
//! ```
//!
//! # Typed display tokens
//!
//! [`tokenize_object`] and [`tokenize_interface`] traverse the input structure
//! directly and return owned [`Token`] sequences, including all layout. Existing
//! render APIs use this same generation path followed by [`to_plain_text`]. The
//! input limits above also bound token allocation: tokens per node, indentation
//! depth and escape expansion are bounded; no partial signature is returned.
//! Token text is WIP-escaped, **not HTML-escaped** or escaped for other destinations.
//! See [`Token`] for the semantic splitting contract; no renderer/theme is supplied.
//!
//! ```
//! use wip_protocol::{InterfaceReference, InterfaceDescriptor, INTERFACE_FORMAT_V1};
//! use wip_text_view::{Interface, TokenKind, tokenize_interface, to_plain_text, render_interface};
//! let reference = InterfaceReference { scope: "/".into(), name: "example".into() };
//! let descriptor = InterfaceDescriptor {
//!     format: INTERFACE_FORMAT_V1.into(), documentation: None,
//!     types: vec![], operations: vec![],
//! };
//! let input = Interface { reference: &reference, descriptor: &descriptor };
//! let tokens = tokenize_interface(&input)?;
//! assert_eq!(tokens[0].kind(), TokenKind::Keyword);
//! assert_eq!(tokens[0].text(), "interface");
//! let concatenated: String = tokens.iter().map(|token| token.text()).collect();
//! assert_eq!(to_plain_text(&tokens), concatenated);
//! assert_eq!(to_plain_text(&tokens), render_interface(&input)?);
//! # Ok::<(), wip_text_view::RenderError>(())
//! ```

#![deny(missing_docs)]

mod limits;
mod render;
mod token;

pub use token::{Token, TokenKind, to_plain_text};

use std::{error::Error, fmt};
use wip_protocol::{
    INTERFACE_FORMAT_V1, InterfaceDescriptor, InterfaceReference, Object, ValidationError,
};

/// One Interface input entity, borrowing its identity and corresponding data.
///
/// The caller pairs values from the same observation/publication. The descriptor
/// itself has no identity, so rendering cannot verify provenance or freshness.
/// This is an SDK display input, not a replacement wire schema or a document.
#[derive(Debug, Clone, Copy)]
pub struct Interface<'a> {
    /// Complete, absolute, Client-facing Interface reference.
    pub reference: &'a InterfaceReference,
    /// Descriptor corresponding to `reference`.
    pub descriptor: &'a InterfaceDescriptor,
}

/// Failure to render a complete, validated signature. No partial text is returned.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RenderError {
    /// Object-independent Protocol validation failed.
    InvalidObject(ValidationError),
    /// The Interface's structured reference is malformed.
    InvalidReference(ValidationError),
    /// Descriptor structure or semantics failed Protocol validation.
    InvalidDescriptor(ValidationError),
    /// A semantically valid descriptor uses a format this renderer cannot interpret.
    UnsupportedDescriptorFormat {
        /// Exact unsupported format identifier.
        format: String,
    },
    /// Input exceeds an implementation safety limit, without truncation.
    ResourceLimit {
        /// Limit that was exceeded.
        limit: ResourceLimit,
    },
}

/// Bounded input dimensions checked before recursive Protocol validation.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ResourceLimit {
    /// Total input string and opaque byte length exceeds 1 MiB.
    InputBytes,
    /// Total declarations, fields, parameters, cases, expressions or references exceeds 16,384.
    Nodes,
    /// More than 256 named type declarations.
    TypeDeclarations,
    /// More than 64 nested TypeExpr levels (outer expression is level one).
    TypeDepth,
}

impl fmt::Display for RenderError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::InvalidObject(e) => write!(f, "invalid Object: {e}"),
            Self::InvalidReference(e) => write!(f, "invalid Interface reference: {e}"),
            Self::InvalidDescriptor(e) => write!(f, "invalid Interface descriptor: {e}"),
            Self::UnsupportedDescriptorFormat { format } => {
                write!(f, "unsupported descriptor format: {format:?}")
            }
            Self::ResourceLimit { limit } => write!(f, "Text View input exceeds {limit:?} limit"),
        }
    }
}

impl Error for RenderError {
    fn source(&self) -> Option<&(dyn Error + 'static)> {
        match self {
            Self::InvalidObject(e) | Self::InvalidReference(e) | Self::InvalidDescriptor(e) => {
                Some(e)
            }
            _ => None,
        }
    }
}

/// Renders one complete Object signature, with no placement path or inferred data.
///
/// Validates shape and ordered-set uniqueness, but cannot validate ancestry or
/// root/non-root placement without context. Missing and empty descriptions remain
/// distinct; ref and validator bytes are ignored for display. Input is unmodified.
/// See [crate documentation](crate) for limits and caller responsibilities.
pub fn render_object(object: &Object) -> Result<String, RenderError> {
    tokenize_object(object).map(|tokens| to_plain_text(&tokens))
}

/// Generates the complete owned display token sequence for one Object signature.
///
/// Uses the same validation, limits, metadata exclusion and caller responsibilities
/// as [`render_object`]. No partial sequence is returned on error. Input is not
/// modified. See [`Token`] and [`TokenKind`] for semantic roles and splitting.
pub fn tokenize_object(object: &Object) -> Result<Vec<Token>, RenderError> {
    limits::object(object)?;
    object.validate().map_err(RenderError::InvalidObject)?;
    Ok(render::object(object))
}

/// Renders one complete Interface signature from one paired input entity.
///
/// Validates reference shape and all descriptor declarations/named references,
/// including cycles, before checking format support. No lookup or Object is
/// needed. All types precede all operations, with original member order; only
/// documentation summaries are shown, without interpreting Host text.
/// Input is unmodified. See [crate documentation](crate) for safety limits.
pub fn render_interface(interface: &Interface<'_>) -> Result<String, RenderError> {
    tokenize_interface(interface).map(|tokens| to_plain_text(&tokens))
}

/// Generates the complete owned display token sequence for one Interface signature.
///
/// Uses the same validation, limits, format support and summary-only documentation
/// as [`render_interface`]. No partial sequence is returned on error. Input is not
/// modified. See [`Token`] and [`TokenKind`] for semantic roles and splitting.
/// This traverses structured declarations directly, not a rendered-text lexer.
pub fn tokenize_interface(interface: &Interface<'_>) -> Result<Vec<Token>, RenderError> {
    limits::interface(interface)?;
    interface
        .reference
        .validate()
        .map_err(RenderError::InvalidReference)?;
    interface
        .descriptor
        .validate()
        .map_err(RenderError::InvalidDescriptor)?;
    if interface.descriptor.format != INTERFACE_FORMAT_V1 {
        return Err(RenderError::UnsupportedDescriptorFormat {
            format: interface.descriptor.format.clone(),
        });
    }
    Ok(render::interface(interface))
}

// Keep both README examples executable as external-consumer doctests.
#[cfg(doctest)]
#[doc = include_str!("../README.md")]
mod readme {}
