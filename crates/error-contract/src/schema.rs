//! The wire DTOs are the schema source. Domain details supply their own JsonSchema.
//! Byte/depth budgets and cross-field validation remain runtime requirements; JSON Schema
//! string lengths count codepoints and cannot replace the codec's UTF-8 byte limits.
use crate::{codec::FailureEnvelope, problem::ProblemDetails};

pub fn failure_schema<D: schemars::JsonSchema>() -> schemars::Schema {
    schemars::schema_for!(FailureEnvelope<D>)
}
pub fn problem_schema<D: schemars::JsonSchema>() -> schemars::Schema {
    schemars::schema_for!(ProblemDetails<D>)
}

/// Existing protocol TS generation consumes this fragment; no second DTO/generator is needed.
#[cfg(feature = "typescript")]
pub fn typescript_declarations() -> String {
    use crate::*;
    use ts_rs::TS;
    let config = ts_rs::Config::default();
    [
        Category::decl(&config),
        OperationContext::decl(&config),
        CorrelationRef::decl(&config),
        CausalLink::decl(&config),
        Diagnostic::decl(&config),
        FailureContext::decl(&config),
        FailureEnvelope::<String>::decl(&config),
        ProblemDetails::<String>::decl(&config),
    ]
    .into_iter()
    .map(|declaration| format!("export {declaration}\n"))
    .collect()
}
