use std::collections::BTreeMap;

use crate::{ProtocolError, ValidationError};

/// Human-readable plain-text documentation attached to a protocol declaration.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct Documentation {
    /// A short, self-contained description.
    pub summary: String,
    /// Optional extended documentation.
    pub details: Option<String>,
}

/// An interface identified by an exact `(scope, name)` pair in one Worldspace.
///
/// Neither field is normalized, case-folded, URL-decoded, or split on display
/// delimiters. Descriptor identity is external to the descriptor itself.
///
/// ```
/// use wip_protocol::{FetchInterfaceRequest, InterfaceReference};
/// let reference = InterfaceReference {
///     scope: "/世::界/#".into(),
///     name: "read::書く#λ".into(),
/// };
/// reference.validate_for_path("/世::界/#/item")?;
/// // Fetching is independent of an Object target path.
/// FetchInterfaceRequest { interface: reference }.validate()?;
/// # Ok::<(), wip_protocol::ValidationError>(())
/// ```
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct InterfaceReference {
    /// Canonical absolute Worldspace path of the interface's scope Object.
    pub scope: String,
    /// Nonempty Unicode local name, with no delimiter restrictions.
    pub name: String,
}

impl InterfaceReference {
    /// Validates reference shape without requiring a target Object path.
    pub fn validate(&self) -> Result<(), ValidationError> {
        crate::validation::validate_interface_reference(self)
    }

    /// Validates shape and that scope is an ancestor-or-self of `path`.
    ///
    /// This is independent of indexability. A Host maps a valid reference's
    /// scope mismatch to `InterfaceMismatch` at the interface-selection stage,
    /// not to the earlier shape-validation `InvalidRequest` stage.
    pub fn validate_for_path(&self, path: &str) -> Result<(), ValidationError> {
        crate::validation::validate_interface_scope(self, path)
    }
}

/// The protocol projection currently published at one Worldspace path.
///
/// `name` is a path segment, `interfaces` is an ordered set of structured interface
/// references, `r#ref` is optional object identity, and `validator` is an
/// independent optional observation precondition. In particular, neither the
/// path nor `r#ref` is a mandatory object identifier.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Object {
    /// Path-segment name. The root object's name is empty.
    pub name: String,
    /// Short public description of the object.
    pub description: Option<String>,
    /// Structured interface references in host publication order.
    pub interfaces: Vec<InterfaceReference>,
    /// Optional opaque identity within one client-facing Worldspace.
    pub r#ref: Option<String>,
    /// Optional opaque validator for the object's observed state.
    pub validator: Option<Vec<u8>>,
}

/// Transport-independent description of one interface.
///
/// Interface identity is supplied by interaction fields such as
/// [`FetchInterfaceResponse::interface`], not embedded in the descriptor.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct InterfaceDescriptor {
    /// Descriptor format version understood by clients and hosts.
    pub format: String,
    /// Optional interface documentation.
    pub documentation: Option<Documentation>,
    /// Named declarations local to this descriptor.
    pub types: Vec<TypeDeclaration>,
    /// Operations exposed by this interface.
    pub operations: Vec<OperationDeclaration>,
}

/// A descriptor-local named type declaration.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TypeDeclaration {
    /// Descriptor-local type name.
    pub name: String,
    /// Optional type documentation.
    pub documentation: Option<Documentation>,
    /// Definition associated with `name`.
    pub definition: TypeExpr,
}

/// A type expression used by declarations, fields, parameters, and returns.
///
/// [`TypeExpr::Named`] references are resolved only within the containing
/// descriptor. Recursive references, including recursion through lists,
/// records, and unions, are invalid.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TypeExpr {
    /// A value carrying no data.
    Unit,
    /// A boolean.
    Boolean,
    /// A JavaScript-lossless signed integer.
    Integer,
    /// A finite IEEE-754 binary64 number.
    Number,
    /// A Unicode string.
    String,
    /// An opaque byte sequence.
    Bytes,
    /// A schema-opaque JSON-compatible value tree.
    Json,
    /// A canonical absolute Worldspace path represented by a string value.
    Entry,
    /// A descriptor-local named declaration.
    Named {
        /// Referenced declaration name.
        name: String,
    },
    /// A record with named fields.
    Record {
        /// Fields in declaration order.
        fields: Vec<FieldDeclaration>,
    },
    /// An ordered list.
    List {
        /// Type of every list item.
        items: Box<TypeExpr>,
    },
    /// A closed set of payload-free cases represented by strings.
    Enum {
        /// Cases in declaration order.
        cases: Vec<EnumCase>,
    },
    /// A discriminated union represented by a record.
    Union {
        /// Cases in declaration order.
        cases: Vec<UnionCase>,
    },
}

/// Declaration of a record field.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FieldDeclaration {
    /// Field name.
    pub name: String,
    /// Whether the field must be present.
    pub required: bool,
    /// Optional field documentation.
    pub documentation: Option<Documentation>,
    /// Field value type.
    pub r#type: TypeExpr,
}

/// One payload-free enum case.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EnumCase {
    /// Case name.
    pub name: String,
    /// Optional case documentation.
    pub documentation: Option<Documentation>,
}

/// One union case with zero or one payload.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct UnionCase {
    /// Case name.
    pub name: String,
    /// Optional case documentation.
    pub documentation: Option<Documentation>,
    /// Optional payload type.
    pub payload: Option<TypeExpr>,
}

/// Declaration of one interface operation.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OperationDeclaration {
    /// Operation name, unique within its descriptor.
    pub name: String,
    /// Optional operation documentation.
    pub documentation: Option<Documentation>,
    /// Parameters in declaration order.
    pub parameters: Vec<ParameterDeclaration>,
    /// Return declaration. No-data operations return [`TypeExpr::Unit`].
    pub returns: ReturnDeclaration,
}

/// Declaration of one operation parameter.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ParameterDeclaration {
    /// Parameter name.
    pub name: String,
    /// Whether the parameter must be present.
    pub required: bool,
    /// Optional parameter documentation.
    pub documentation: Option<Documentation>,
    /// Parameter value type.
    pub r#type: TypeExpr,
}

/// Declaration of an operation return value.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ReturnDeclaration {
    /// Optional return-value documentation.
    pub documentation: Option<Documentation>,
    /// Return value type.
    pub r#type: TypeExpr,
}

/// A schema-neutral logical protocol value.
///
/// Enum cases, union cases, entries, and JSON are interpretations imposed by a
/// descriptor. They are deliberately not variants of this enum. Record key
/// ordering is not protocol-significant.
#[derive(Debug, Clone, PartialEq)]
pub enum Value {
    /// A value carrying no data; it also represents JSON null under `Json`.
    Unit,
    /// A boolean.
    Boolean(bool),
    /// A JavaScript-lossless integer.
    Integer(i64),
    /// A finite IEEE-754 binary64 number.
    Number(f64),
    /// A Unicode string.
    String(String),
    /// An opaque byte sequence.
    Bytes(Vec<u8>),
    /// String-keyed fields with order-insensitive semantics.
    Record(BTreeMap<String, Value>),
    /// An ordered sequence.
    List(Vec<Value>),
}

impl Value {
    /// Returns the structural kind used by validation diagnostics.
    #[must_use]
    pub fn kind(&self) -> crate::ValueKind {
        match self {
            Self::Unit => crate::ValueKind::Unit,
            Self::Boolean(_) => crate::ValueKind::Boolean,
            Self::Integer(_) => crate::ValueKind::Integer,
            Self::Number(_) => crate::ValueKind::Number,
            Self::String(_) => crate::ValueKind::String,
            Self::Bytes(_) => crate::ValueKind::Bytes,
            Self::Record(_) => crate::ValueKind::Record,
            Self::List(_) => crate::ValueKind::List,
        }
    }
}

/// Request for the object currently published at one path and its indexable descendants.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ObserveRequest {
    /// Canonical absolute path of the observation root.
    pub path: String,
    /// Maximum observed depth, where the requested object has depth zero.
    pub depth: u32,
}

/// One node in an object observation.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ObjectObservation {
    /// Object observed at this Worldspace position.
    pub object: Object,
    /// Complete direct children when observed, or `None` at the requested depth boundary.
    pub children: Option<Vec<ObjectObservation>>,
}

/// Successful `observe` response.
pub type ObserveResponse = ObjectObservation;

/// Request for the current descriptor of one structured interface reference.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FetchInterfaceRequest {
    /// Structured interface reference obtained from an object.
    pub interface: InterfaceReference,
}

/// Successful `fetch_interface` response.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FetchInterfaceResponse {
    /// Structured interface reference corresponding to the request.
    pub interface: InterfaceReference,
    /// Optional opaque ref of the scope Object, not part of interface identity.
    pub scope_ref: Option<String>,
    /// Current validated descriptor.
    pub descriptor: InterfaceDescriptor,
    /// Optional opaque validator for this descriptor observation.
    pub validator: Option<Vec<u8>>,
}

/// Object portion of an operation target.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Target {
    /// Canonical absolute path of the target object.
    pub path: String,
    /// Optional validator copied from the object observation.
    pub validator: Option<Vec<u8>>,
}

/// Interface portion of an operation target.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct InterfaceTarget {
    /// Structured reference selected from the object's interface set.
    pub reference: InterfaceReference,
    /// Optional scope Object ref copied from the same interface observation.
    pub scope_ref: Option<String>,
    /// Optional validator copied from the interface observation.
    pub validator: Option<Vec<u8>>,
}

/// Transport-independent request to execute one operation.
#[derive(Debug, Clone, PartialEq)]
pub struct CallOperationRequest {
    /// Path and optional object-state precondition.
    pub target: Target,
    /// Interface reference and its independent optional precondition.
    pub interface: InterfaceTarget,
    /// Operation name within the selected descriptor.
    pub operation: String,
    /// Named arguments; map iteration order has no protocol meaning.
    pub arguments: BTreeMap<String, Value>,
}

/// Successful operation response.
#[derive(Debug, Clone, PartialEq)]
pub struct CallOperationResponse {
    /// Application result validated against the operation return declaration.
    pub result: Value,
    /// Optional validator for the object state observed after execution.
    pub validator: Option<Vec<u8>>,
}

/// Logical outcome of any core interaction.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ProtocolResult<T> {
    /// The interaction succeeded.
    Success(T),
    /// The interaction failed at the protocol boundary.
    Failure(ProtocolError),
}

impl Object {
    /// Validates the object-independent name and ordered-interface-set rules.
    pub fn validate(&self) -> Result<(), ValidationError> {
        crate::validation::validate_object(self)
    }

    /// Validates this object as the response observed at `path`.
    pub fn validate_at_path(&self, path: &str) -> Result<(), ValidationError> {
        crate::validation::validate_object_at_path(self, path)
    }

    /// Validates equal representations for a shared ref and present validator.
    pub fn validate_consistency_with(&self, other: &Self) -> Result<(), ValidationError> {
        crate::validation::validate_object_consistency(self, other)
    }
}

impl InterfaceDescriptor {
    /// Validates declaration structure, named references, and absence of cycles.
    pub fn validate(&self) -> Result<(), ValidationError> {
        crate::validation::validate_descriptor(self)
    }

    /// Validates a raw value against a type expression in this descriptor.
    pub fn validate_value(&self, r#type: &TypeExpr, value: &Value) -> Result<(), ValidationError> {
        crate::validation::validate_root_value(self, r#type, value)
    }

    /// Validates named arguments for one declared operation.
    pub fn validate_arguments(
        &self,
        operation: &str,
        arguments: &BTreeMap<String, Value>,
    ) -> Result<(), ValidationError> {
        crate::validation::validate_arguments(self, operation, arguments)
    }

    /// Validates one operation result against its return declaration.
    pub fn validate_result(&self, operation: &str, result: &Value) -> Result<(), ValidationError> {
        crate::validation::validate_result(self, operation, result)
    }

    /// Validates the path, operation, and arguments in a call request.
    pub fn validate_call(&self, request: &CallOperationRequest) -> Result<(), ValidationError> {
        crate::validation::validate_call(self, request)
    }
}

impl ObserveRequest {
    /// Validates the request path.
    pub fn validate(&self) -> Result<(), ValidationError> {
        crate::validation::validate_path(&self.path)
    }

    /// Validates observation shape, names, and requested depth.
    ///
    /// Failure means the Client received an invalid success response; it is not a
    /// Host-returned [`crate::ProtocolErrorCode`].
    pub fn validate_response(&self, response: &ObserveResponse) -> Result<(), ValidationError> {
        crate::validation::validate_observe_response(response, self)
    }
}

impl FetchInterfaceRequest {
    /// Validates reference shape; fetching an interface needs no target path.
    pub fn validate(&self) -> Result<(), ValidationError> {
        self.interface.validate()
    }
}

impl FetchInterfaceResponse {
    /// Validates request correspondence and descriptor semantics.
    ///
    /// Failure means the Client received an invalid success response; it is not a
    /// Host-returned [`crate::ProtocolErrorCode`]. Descriptor format support is a
    /// separate Client concern: an otherwise valid descriptor with an unsupported
    /// `format` is a Client-local `UnsupportedDescriptorFormat`.
    pub fn validate_for(&self, request: &FetchInterfaceRequest) -> Result<(), ValidationError> {
        crate::validation::validate_fetch_interface_response(self, request)
    }
}

impl CallOperationRequest {
    /// Validates descriptor-independent request shape only.
    ///
    /// Scope compatibility is deliberately deferred to interface selection so
    /// that `NotFound` precedes `InterfaceMismatch` at the Host boundary.
    pub fn validate(&self) -> Result<(), ValidationError> {
        crate::validation::validate_call_shape(self)
    }

    /// Validates this call's shape and descriptor-bound operation/arguments.
    /// Scope compatibility and membership require Host publication context.
    pub fn validate_with(&self, descriptor: &InterfaceDescriptor) -> Result<(), ValidationError> {
        descriptor.validate_call(self)
    }
}

impl CallOperationResponse {
    /// Validates this result against the named operation in `descriptor`.
    ///
    /// Failure means the Client received an invalid success response; it is not a
    /// Host-returned [`crate::ProtocolErrorCode`].
    pub fn validate_for(
        &self,
        descriptor: &InterfaceDescriptor,
        operation: &str,
    ) -> Result<(), ValidationError> {
        descriptor.validate_result(operation, &self.result)
    }
}
