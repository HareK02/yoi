use std::error::Error;
use std::fmt::{self, Display, Formatter};

/// Namespace in which a declaration name must be unique.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum NameNamespace {
    /// Named types in an interface descriptor.
    TypeDeclaration,
    /// Operations in an interface descriptor.
    Operation,
    /// Fields in an inline or named record type.
    Field,
    /// Parameters in an operation declaration.
    Parameter,
    /// Cases in an enum type.
    EnumCase,
    /// Cases in a union type.
    UnionCase,
    /// Child object names beneath one tree node.
    ChildObject,
}

/// Structural kind of a schema-neutral logical value.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ValueKind {
    /// Unit value.
    Unit,
    /// Boolean value.
    Boolean,
    /// Integer value.
    Integer,
    /// Floating-point number value.
    Number,
    /// String value.
    String,
    /// Byte sequence.
    Bytes,
    /// Record value.
    Record,
    /// List value.
    List,
}

/// A typed segment identifying a declaration or value location.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PathSegment {
    /// Descriptor root.
    Descriptor,
    /// Named type declaration.
    Declaration(String),
    /// Operation declaration or call.
    Operation(String),
    /// Record field declaration or value.
    Field(String),
    /// Operation parameter or argument.
    Parameter(String),
    /// Enum or union case.
    Case(String),
    /// List index.
    Index(usize),
    /// Operation return value.
    Return,
    /// Object at a Worldspace path.
    Object(String),
    /// Child object in an observation response.
    Child(String),
}

/// Machine-readable reason why logical protocol validation failed.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ValidationErrorKind {
    /// A name occurred more than once in one namespace.
    DuplicateName {
        /// Namespace containing the duplicate.
        namespace: NameNamespace,
        /// Duplicate name.
        name: String,
    },
    /// A named type was not declared in the descriptor.
    UnknownType {
        /// Missing descriptor-local type name.
        name: String,
    },
    /// Named declarations form a recursive cycle.
    RecursiveType {
        /// Cycle in traversal order, with the starting name repeated last.
        cycle: Vec<String>,
    },
    /// A value has the wrong structural kind.
    TypeMismatch {
        /// Expected kind.
        expected: ValueKind,
        /// Actual kind.
        actual: ValueKind,
    },
    /// A record value contains an undeclared field.
    UnknownField {
        /// Unknown field name.
        name: String,
    },
    /// A record value omitted a required field.
    MissingField {
        /// Missing field name.
        name: String,
    },
    /// An enum or union selected an undeclared case.
    UnknownCase {
        /// Unknown case name.
        name: String,
    },
    /// A payload-bearing union case omitted `value`.
    MissingUnionPayload,
    /// A payload-free union case supplied `value`.
    UnexpectedUnionPayload,
    /// An operation call contains an undeclared argument.
    UnknownParameter {
        /// Unknown parameter name.
        name: String,
    },
    /// An operation call omitted a required argument.
    MissingParameter {
        /// Missing parameter name.
        name: String,
    },
    /// An operation is not declared by the descriptor.
    UnknownOperation {
        /// Unknown operation name.
        name: String,
    },
    /// An integer is outside the interoperable safe range.
    IntegerOutOfRange {
        /// Rejected integer.
        value: i64,
    },
    /// A numeric value is NaN or infinite.
    NonFiniteNumber,
    /// A JSON-interpreted value tree contains protocol bytes.
    BytesInJson,
    /// An object name is not a valid root or non-root path segment.
    InvalidObjectName {
        /// Rejected object name.
        name: String,
    },
    /// An object's ordered interface set contains a duplicate.
    DuplicateInterface {
        /// Duplicate structured interface reference.
        reference: crate::InterfaceReference,
    },
    /// The local interface name is empty.
    EmptyInterfaceName,
    /// A valid scope is not an ancestor-or-self of the target path.
    InterfaceScopeMismatch {
        /// Selected scope path.
        scope: String,
        /// Target Object path.
        path: String,
    },
    /// A string is not a canonical absolute Worldspace path.
    InvalidPath {
        /// Rejected path.
        path: String,
    },
    /// An object name does not match the path where it was observed.
    ObjectNameMismatch {
        /// Name required by the observation path.
        expected: String,
        /// Name supplied by the object.
        actual: String,
    },
    /// Equivalent ref/validator observations disagree on object representation.
    InconsistentObjectObservation {
        /// Shared opaque object reference.
        reference: String,
    },
    /// An observation omitted children before reaching the requested depth.
    ChildrenMissingBeforeDepth {
        /// Node depth where children were required.
        depth: u32,
    },
    /// An observation included children at the requested depth boundary.
    ChildrenAtDepthBoundary {
        /// Boundary depth where children must be unobserved.
        depth: u32,
    },
    /// A response refers to a different interface than its request.
    InterfaceResponseMismatch {
        /// Requested structured reference (boxed to keep validation errors small).
        expected: Box<crate::InterfaceReference>,
        /// Returned structured reference (boxed to keep validation errors small).
        actual: Box<crate::InterfaceReference>,
    },
}

/// A validation failure with a typed path to the failing location.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ValidationError {
    /// Path from the validation root to the failure.
    pub path: Vec<PathSegment>,
    /// Machine-readable failure reason.
    pub kind: ValidationErrorKind,
}

impl ValidationError {
    pub(crate) fn new(kind: ValidationErrorKind) -> Self {
        Self {
            path: Vec::new(),
            kind,
        }
    }

    pub(crate) fn prepend(mut self, segment: PathSegment) -> Self {
        self.path.insert(0, segment);
        self
    }
}

impl Display for ValidationError {
    fn fmt(&self, formatter: &mut Formatter<'_>) -> fmt::Result {
        if self.path.is_empty() {
            write!(formatter, "protocol validation failed: {:?}", self.kind)
        } else {
            write!(
                formatter,
                "protocol validation failed at {:?}: {:?}",
                self.path, self.kind
            )
        }
    }
}

impl Error for ValidationError {}

/// One core interaction whose failure codes form a closed set.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ProtocolInteraction {
    /// Observe one object and its indexable descendants to a requested depth.
    Observe,
    /// Fetch one interface descriptor by its structured reference.
    FetchInterface,
    /// Invoke one operation on a path-targeted object.
    CallOperation,
}

/// Stable protocol failure code returned by a host.
///
/// This is the complete wire-level code set. A Client treats a malformed success
/// or failure response, including a code disallowed for its interaction, as a
/// Client-local `InvalidResponse`. An unsupported descriptor format is likewise
/// a Client-local `UnsupportedDescriptorFormat`; neither condition is a code a
/// Host can return.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ProtocolErrorCode {
    /// The request is structurally invalid before an operation descriptor is fixed.
    InvalidRequest,
    /// The target object is not published.
    NotFound,
    /// The interface reference is not currently published.
    InterfaceNotFound,
    /// The selected interface is not on the current object.
    InterfaceMismatch,
    /// The selected operation is not declared by the current interface.
    OperationNotFound,
    /// Supplied arguments do not match the fixed operation declaration.
    InvalidArguments,
    /// The operation requires an object validator and none was supplied.
    ValidatorRequired,
    /// The supplied object validator does not match current state.
    ValidatorMismatch,
    /// A mutable interface requires its validator and none was supplied.
    InterfaceValidatorRequired,
    /// The supplied interface validator does not match the descriptor.
    InterfaceValidatorMismatch,
    /// A published operation is not permitted for the caller.
    PermissionDenied,
    /// A bounded protocol resource limit was exceeded.
    ///
    /// For `call_operation`, the Host guarantees that the operation was not run
    /// or that all of its effects were rolled back.
    ResourceLimitExceeded,
    /// The Host encountered an internal failure.
    ///
    /// For `call_operation`, the Host guarantees that the operation was not run
    /// or that all of its effects were rolled back.
    Internal,
    /// The Host cannot guarantee that the operation was not run or rolled back.
    ///
    /// A Client must not automatically retry because the operation may have
    /// committed effects even though no success response was available.
    OperationOutcomeUnknown,
}

impl ProtocolErrorCode {
    /// Every canonical wire-level error code.
    pub const ALL: [Self; 14] = [
        Self::InvalidRequest,
        Self::NotFound,
        Self::InterfaceNotFound,
        Self::InterfaceMismatch,
        Self::OperationNotFound,
        Self::InvalidArguments,
        Self::ValidatorRequired,
        Self::ValidatorMismatch,
        Self::InterfaceValidatorRequired,
        Self::InterfaceValidatorMismatch,
        Self::PermissionDenied,
        Self::ResourceLimitExceeded,
        Self::Internal,
        Self::OperationOutcomeUnknown,
    ];

    /// Whether a Host may return this code for `interaction`.
    ///
    /// `false` identifies an invalid error response; it does not turn that
    /// response into another wire-level protocol failure.
    #[must_use]
    pub const fn is_allowed_for(self, interaction: ProtocolInteraction) -> bool {
        use ProtocolErrorCode::{
            InterfaceMismatch, InterfaceNotFound, InterfaceValidatorMismatch,
            InterfaceValidatorRequired, Internal, InvalidArguments, InvalidRequest, NotFound,
            OperationNotFound, OperationOutcomeUnknown, PermissionDenied, ResourceLimitExceeded,
            ValidatorMismatch, ValidatorRequired,
        };
        use ProtocolInteraction::{CallOperation, FetchInterface, Observe};

        match (interaction, self) {
            (_, InvalidRequest | ResourceLimitExceeded | Internal) => true,
            (Observe, NotFound) => true,
            (FetchInterface, InterfaceNotFound) => true,
            (
                CallOperation,
                NotFound
                | InterfaceMismatch
                | OperationNotFound
                | InvalidArguments
                | ValidatorRequired
                | ValidatorMismatch
                | InterfaceValidatorRequired
                | InterfaceValidatorMismatch
                | PermissionDenied
                | OperationOutcomeUnknown,
            ) => true,
            (
                _,
                NotFound
                | InterfaceNotFound
                | InterfaceMismatch
                | OperationNotFound
                | InvalidArguments
                | ValidatorRequired
                | ValidatorMismatch
                | InterfaceValidatorRequired
                | InterfaceValidatorMismatch
                | PermissionDenied
                | OperationOutcomeUnknown,
            ) => false,
        }
    }
}

/// Transport-independent protocol failure.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProtocolError {
    /// Stable programmatic code. Consumers must branch on this field.
    pub code: ProtocolErrorCode,
    /// Bounded human-readable diagnostic. Consumers must not branch on it.
    pub message: String,
}

impl Display for ProtocolError {
    fn fmt(&self, formatter: &mut Formatter<'_>) -> fmt::Result {
        write!(
            formatter,
            "protocol failure ({:?}): {}",
            self.code, self.message
        )
    }
}

impl Error for ProtocolError {}
