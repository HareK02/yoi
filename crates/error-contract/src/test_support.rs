//! Shared executable examples, not production authorization/domain adapters.
//! The fixture authorization context stands in for a boundary's current authenticated policy
//! decision; it must never be reconstructed from a received failure or an audience label.
use crate::*;
use serde::{Deserialize, Serialize};

pub const SUBJECT_FIXTURE: &str = include_str!("../fixtures/subject-denied.json");
pub const WORKDIR_FIXTURE: &str = include_str!("../fixtures/workdir-in-use.json");
pub const FIXTURE_SECRETS: &[&str] = &[
    "credential-secret",
    "/private/worktree",
    "raw-http-body",
    "internal-source",
];

#[derive(Clone, Copy)]
pub enum Language {
    English,
    Japanese,
}
/// Fixture policy for subject-1/workdir-1 only; a true visibility decision is not a grant
/// to disclose arbitrary resource references supplied by a received wire payload.
pub struct CurrentAccess {
    pub can_view_resource: bool,
    pub language: Language,
}
impl CurrentAccess {
    fn visible_reference(&self, reference: Option<&str>, expected: &str) -> Option<String> {
        reference
            .filter(|value| self.can_view_resource && *value == expected)
            .map(str::to_owned)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
#[cfg_attr(feature = "json-schema", derive(schemars::JsonSchema))]
#[cfg_attr(feature = "typescript", derive(ts_rs::TS))]
pub enum RequiredOperation {
    #[serde(rename = "session.record")]
    SessionRecord,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
#[cfg_attr(feature = "json-schema", derive(schemars::JsonSchema))]
#[cfg_attr(feature = "typescript", derive(ts_rs::TS))]
pub struct SubjectDenied {
    pub required_operation: RequiredOperation,
    pub subject_ref: Option<String>,
}
impl FailureCause for SubjectDenied {
    fn identity(&self) -> FailureIdentity {
        FailureIdentity {
            domain: "subject",
            reason: "operation_denied",
            category: Category::Authorization,
            details_version: 1,
        }
    }
    fn validate(&self) -> Result<(), ContractError> {
        if let Some(reference) = &self.subject_ref {
            validate_identifier(reference)?;
        }
        Ok(())
    }
}
impl WireDetails for SubjectDenied {
    fn supports(domain: &str, reason: &str, version: u16) -> bool {
        domain == "subject" && reason == "operation_denied" && version == 1
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
#[cfg_attr(feature = "json-schema", derive(schemars::JsonSchema))]
#[cfg_attr(feature = "typescript", derive(ts_rs::TS))]
pub enum WorkdirOperation {
    Open,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
#[cfg_attr(feature = "json-schema", derive(schemars::JsonSchema))]
#[cfg_attr(feature = "typescript", derive(ts_rs::TS))]
pub struct WorkdirInUse {
    pub operation: WorkdirOperation,
    pub workdir_ref: Option<String>,
}
impl FailureCause for WorkdirInUse {
    fn identity(&self) -> FailureIdentity {
        FailureIdentity {
            domain: "workdir",
            reason: "in_use",
            category: Category::Conflict,
            details_version: 1,
        }
    }
    fn validate(&self) -> Result<(), ContractError> {
        if let Some(reference) = &self.workdir_ref {
            validate_identifier(reference)?;
        }
        Ok(())
    }
}
impl WireDetails for WorkdirInUse {
    fn supports(domain: &str, reason: &str, version: u16) -> bool {
        domain == "workdir" && reason == "in_use" && version == 1
    }
}

#[derive(Debug, thiserror::Error)]
#[error("internal-source credential-secret /private/worktree raw-http-body")]
pub struct PrivateSource;

#[derive(Debug, thiserror::Error)]
#[error("subject rejected operation")]
pub struct SubjectError {
    pub details: SubjectDenied,
    #[source]
    pub source: PrivateSource,
}
#[derive(Debug, thiserror::Error)]
#[error("workdir in use")]
pub struct WorkdirError {
    pub details: WorkdirInUse,
    #[source]
    pub source: PrivateSource,
}
macro_rules! cause {
    ($name:ty) => {
        impl FailureCause for $name {
            fn identity(&self) -> FailureIdentity {
                self.details.identity()
            }
            fn validate(&self) -> Result<(), ContractError> {
                self.details.validate()
            }
        }
    };
}
cause!(SubjectError);
cause!(WorkdirError);

// The domain selects both details and common metadata. No raw source/Display is inspected.
fn allowed_context(context: &FailureContext, access: &CurrentAccess) -> FailureContext {
    FailureContext {
        operations: context
            .operations
            .iter()
            .filter_map(|operation| {
                if !matches!(
                    operation.operation.as_str(),
                    "worker.create" | "workdir.open"
                ) || !matches!(operation.stage.as_str(), "register" | "prepare")
                {
                    return None;
                }
                Some(OperationContext {
                    operation: operation.operation.clone(),
                    stage: operation.stage.clone(),
                    operation_ref: access.visible_reference(
                        operation.operation_ref.as_deref(),
                        "visible-operation-ref",
                    ),
                })
            })
            .collect(),
        // A wire correlation is not a local proof. This fixture boundary knows only these
        // two safe occurrence references; production adapters use their current binding.
        correlation: context
            .correlation
            .as_ref()
            .filter(|reference| matches!(reference.as_str(), "correlation-1" | "correlation-2"))
            .cloned(),
        supplementary_omitted: !context.diagnostics.is_empty()
            || !context.causes.is_empty()
            || context.supplementary_omitted,
        ..FailureContext::default()
    }
}
fn subject_disclosure(
    details: &SubjectDenied,
    context: &FailureContext,
    access: &CurrentAccess,
) -> Result<Disclosure<SubjectDenied>, ContractError> {
    let details = SubjectDenied {
        required_operation: details.required_operation,
        subject_ref: access.visible_reference(details.subject_ref.as_deref(), "subject-1"),
    };
    let explanation = match access.language {
        Language::English => compose_explanation(
            "session.record is denied.",
            "Worker creation cannot register the session",
        )?,
        Language::Japanese => compose_explanation(
            "session.record が拒否されました。",
            "Worker作成でSessionを登録できません",
        )?,
    };
    Ok(Disclosure {
        details,
        explanation,
        context: allowed_context(context, access),
    })
}
fn workdir_disclosure(
    details: &WorkdirInUse,
    context: &FailureContext,
    access: &CurrentAccess,
) -> Disclosure<WorkdirInUse> {
    let details = WorkdirInUse {
        operation: details.operation,
        workdir_ref: access.visible_reference(details.workdir_ref.as_deref(), "workdir-1"),
    };
    let explanation = match access.language {
        Language::English => "Workdir open cannot proceed because the workdir is in use.",
        Language::Japanese => "Workdirは使用中のため開けません。",
    };
    Disclosure {
        details,
        explanation: explanation.into(),
        context: allowed_context(context, access),
    }
}
macro_rules! adapters {
    ($($audience:ty),*) => { $(
        impl ProjectFor<$audience> for SubjectDenied {
            type Authorization = CurrentAccess; type Details = SubjectDenied;
            fn disclose(&self, facts: &FailureFacts<Self>, access: &CurrentAccess) -> Result<Disclosure<Self::Details>, ContractError> {
                subject_disclosure(self, facts.context(), access)
            }
        }
        impl ProjectFor<$audience> for WorkdirInUse {
            type Authorization = CurrentAccess; type Details = WorkdirInUse;
            fn disclose(&self, facts: &FailureFacts<Self>, access: &CurrentAccess) -> Result<Disclosure<Self::Details>, ContractError> {
                Ok(workdir_disclosure(self, facts.context(), access))
            }
        }
        impl ProjectFor<$audience> for SubjectError {
            type Authorization = CurrentAccess; type Details = SubjectDenied;
            fn disclose(&self, facts: &FailureFacts<Self>, access: &CurrentAccess) -> Result<Disclosure<Self::Details>, ContractError> {
                subject_disclosure(&self.details, facts.context(), access)
            }
        }
        impl ProjectFor<$audience> for WorkdirError {
            type Authorization = CurrentAccess; type Details = WorkdirInUse;
            fn disclose(&self, facts: &FailureFacts<Self>, access: &CurrentAccess) -> Result<Disclosure<Self::Details>, ContractError> {
                Ok(workdir_disclosure(&self.details, facts.context(), access))
            }
        }
    )* };
}
adapters!(Browser, Tool, Storage, InternalTransfer, Telemetry);

pub fn subject_facts() -> FailureFacts<SubjectError> {
    let mut facts = FailureFacts::new(SubjectError {
        details: SubjectDenied {
            required_operation: RequiredOperation::SessionRecord,
            subject_ref: Some("subject-1".into()),
        },
        source: PrivateSource,
    })
    .unwrap();
    facts
        .with_context(OperationContext::new("worker.create", "register").unwrap())
        .unwrap();
    facts
        .set_correlation(CorrelationRef::new("correlation-1").unwrap())
        .unwrap();
    facts
}
pub fn workdir_facts() -> FailureFacts<WorkdirError> {
    let mut facts = FailureFacts::new(WorkdirError {
        details: WorkdirInUse {
            operation: WorkdirOperation::Open,
            workdir_ref: Some("workdir-1".into()),
        },
        source: PrivateSource,
    })
    .unwrap();
    facts
        .with_context(OperationContext::new("workdir.open", "prepare").unwrap())
        .unwrap();
    facts
        .set_correlation(CorrelationRef::new("correlation-2").unwrap())
        .unwrap();
    facts
}

/// Inner Subject API vs outer Worker API deliberately choose different HTTP classifications.
pub struct SubjectApi;
pub struct WorkerApi;
pub struct WorkdirApi;
impl crate::problem::ApiBoundary<SubjectDenied> for SubjectApi {
    fn classify(&self, _: &SubjectDenied) -> crate::problem::ProblemClass {
        crate::problem::ProblemClass::authorization_denied()
    }
}
impl crate::problem::ApiBoundary<SubjectDenied> for WorkerApi {
    fn classify(&self, _: &SubjectDenied) -> crate::problem::ProblemClass {
        crate::problem::ProblemClass::conflict()
    }
}
impl crate::problem::ApiBoundary<WorkdirInUse> for WorkdirApi {
    fn classify(&self, _: &WorkdirInUse) -> crate::problem::ProblemClass {
        crate::problem::ProblemClass::conflict()
    }
}

pub fn assert_no_fixture_secrets(bytes: &[u8]) {
    let text = std::str::from_utf8(bytes).expect("JSON is UTF-8");
    for secret in FIXTURE_SECRETS {
        assert!(!text.contains(secret), "secret escaped the allowlist");
    }
}

/// Check the field allowlist on the same wire fixture used by HTTP/TS/storage consumers.
pub fn assert_detail_allowlist(bytes: &[u8], allowed: &[&str]) {
    let value: serde_json::Value = serde_json::from_slice(bytes).expect("fixture JSON");
    let details = value["details"]
        .as_object()
        .expect("fixture details object");
    for key in details.keys() {
        assert!(
            allowed.contains(&key.as_str()),
            "unexpected public detail field: {key}"
        );
    }
}

pub fn assert_context_preserves_details<D: WireDetails + Clone + PartialEq + std::fmt::Debug>(
    facts: &mut FailureFacts<D>,
    context: OperationContext,
) {
    let original = facts.cause().clone();
    facts
        .with_context(context)
        .expect("fixture context fits budget");
    assert_eq!(facts.cause(), &original);
}

/// Semantic check shared by Rust/HTTP/storage fixture consumers. Compare typed details,
/// not translated prose or diagnostic order. Reprojection is intentionally a separate step.
pub fn assert_semantic_roundtrip<D: WireDetails + PartialEq + std::fmt::Debug>(
    bytes: &[u8],
    expected: &D,
    budget: Budget,
) {
    match decode_failure::<D>(bytes, budget) {
        DecodeFailure::Known(received) => assert_eq!(received.facts().cause(), expected),
        _ => panic!("expected a known validated fixture"),
    }
}
