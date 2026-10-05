use std::collections::{BTreeMap, BTreeSet};
use std::fmt;
use std::sync::Arc;

use async_trait::async_trait;
use protocol::{
    CompletionEntry, FeatureInvocation, FeatureInvocationDescriptor, FeatureInvocationIdentity,
    FeatureInvocationResult, InvocationCompletion, InvocationValue, validate_feature_invocation,
};

use super::{FeatureId, FeatureInstallError};

pub(crate) const MAX_INVOCATION_MESSAGE_BYTES: usize = 4 * 1024;
pub(crate) const MAX_INVOCATION_CONTEXT_BYTES: usize = 16 * 1024;
const MAX_COMPLETION_ENTRIES: usize = 64;
const MAX_COMPLETION_VALUE_BYTES: usize = 1024;
const MAX_COMPLETION_DESCRIPTION_BYTES: usize = 2 * 1024;
const MAX_COMPLETION_USAGE_BYTES: usize = 4 * 1024;
const MAX_INVOCATION_DESCRIPTOR_BYTES: usize =
    protocol::invocation::MAX_FEATURE_INVOCATION_DESCRIPTOR_BYTES;
const MAX_COMPLETION_RESPONSE_BYTES: usize = 64 * 1024;

pub(crate) fn invocation_result_is_bounded(result: &FeatureInvocationResult) -> bool {
    result.message.len() <= MAX_INVOCATION_MESSAGE_BYTES
        && result
            .context
            .as_ref()
            .is_none_or(|context| context.len() <= MAX_INVOCATION_CONTEXT_BYTES)
}

fn truncate_display(text: &mut String, max_bytes: usize) {
    let mut end = text.len().min(max_bytes);
    while !text.is_char_boundary(end) {
        end -= 1;
    }
    text.truncate(end);
}

/// Values are never truncated: doing so could silently select a different
/// operation. Display-only text may be shortened; oversized metadata is dropped.
pub(crate) fn bounded_completion_entries(
    entries: impl IntoIterator<Item = CompletionEntry>,
) -> Vec<CompletionEntry> {
    let mut result = Vec::new();
    let mut bytes = 2; // JSON array delimiters.
    for mut entry in entries.into_iter().take(MAX_COMPLETION_ENTRIES * 4) {
        if entry.value.len() > MAX_COMPLETION_VALUE_BYTES {
            continue;
        }
        if let Some(description) = &mut entry.description {
            truncate_display(description, MAX_COMPLETION_DESCRIPTION_BYTES);
        }
        if let Some(usage) = &mut entry.usage {
            truncate_display(usage, MAX_COMPLETION_USAGE_BYTES);
        }
        if entry.invocation.as_ref().is_some_and(|descriptor| {
            serde_json::to_vec(descriptor).map_or(true, |encoded| {
                encoded.len() > MAX_INVOCATION_DESCRIPTOR_BYTES
            })
        }) {
            continue;
        }
        let Ok(encoded) = serde_json::to_vec(&entry) else {
            continue;
        };
        let additional = encoded.len() + usize::from(!result.is_empty());
        if bytes + additional > MAX_COMPLETION_RESPONSE_BYTES {
            continue;
        }
        bytes += additional;
        result.push(entry);
        if result.len() == MAX_COMPLETION_ENTRIES {
            break;
        }
    }
    result
}

/// Host-owned execution context. `invocation_id` is the idempotency key a
/// handler must use when its operation has external side effects. Workspace,
/// credential, and permission authority remain captured by the handler itself.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FeatureInvocationContext {
    pub invocation_id: String,
}

#[async_trait]
pub trait FeatureInvocationHandler: Send + Sync {
    /// Side-effect-free runtime availability/permission check. Installation is
    /// necessary but not sufficient: discovery and execution both consult this.
    fn is_available(&self) -> bool {
        true
    }

    /// Side-effect-free semantic and authority validation. Installed handlers
    /// retain Workspace/credential/permission authority; request arguments never
    /// supply it. The Worker calls this before persisting an execution start
    /// and before reusing successful preparation context on replay or resume.
    fn validate(
        &self,
        _invocation: &FeatureInvocation,
    ) -> Result<(), FeatureInvocationHandlerError> {
        Ok(())
    }

    async fn invoke(
        &self,
        context: FeatureInvocationContext,
        invocation: &FeatureInvocation,
    ) -> Result<FeatureInvocationResult, FeatureInvocationHandlerError>;
}

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[error("{message}")]
pub struct FeatureInvocationHandlerError {
    pub message: String,
    /// True only when the handler cannot determine whether its business-side
    /// effect committed. The Worker never automatically retries this outcome.
    pub outcome_unknown: bool,
}

impl FeatureInvocationHandlerError {
    pub fn failed(message: impl Into<String>) -> Self {
        Self {
            message: message.into(),
            outcome_unknown: false,
        }
    }

    pub fn outcome_unknown(message: impl Into<String>) -> Self {
        Self {
            message: message.into(),
            outcome_unknown: true,
        }
    }
}

#[async_trait]
pub trait FeatureInvocationCompletionProvider: Send + Sync {
    /// The provider selector comes only from installed host metadata, never
    /// from completion-request input. Providers retain their own scoped authority.
    async fn complete(
        &self,
        completion: &InvocationCompletion,
        argument: &str,
        prefix: &str,
    ) -> Vec<CompletionEntry>;
}

#[derive(Clone)]
struct RegisteredInvocation {
    descriptor: FeatureInvocationDescriptor,
    handler: Arc<dyn FeatureInvocationHandler>,
    completion_provider: Option<Arc<dyn FeatureInvocationCompletionProvider>>,
}

/// Installed invocation metadata and executable handlers. Equality/debug expose
/// only public descriptors; implementation objects and credentials never leak.
#[derive(Clone, Default)]
pub struct FeatureInvocationRegistry {
    entries: BTreeMap<FeatureInvocationIdentity, RegisteredInvocation>,
    names: BTreeMap<String, FeatureInvocationIdentity>,
}

impl fmt::Debug for FeatureInvocationRegistry {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("FeatureInvocationRegistry")
            .field("descriptors", &self.descriptors())
            .finish()
    }
}

impl PartialEq for FeatureInvocationRegistry {
    fn eq(&self, other: &Self) -> bool {
        self.descriptors() == other.descriptors()
    }
}
impl Eq for FeatureInvocationRegistry {}

impl FeatureInvocationRegistry {
    pub fn descriptors(&self) -> Vec<FeatureInvocationDescriptor> {
        self.entries
            .values()
            .filter(|entry| entry.handler.is_available())
            .map(|entry| entry.descriptor.clone())
            .collect()
    }

    pub fn descriptor(
        &self,
        identity: &FeatureInvocationIdentity,
    ) -> Option<&FeatureInvocationDescriptor> {
        self.entries
            .get(identity)
            .filter(|entry| entry.handler.is_available())
            .map(|entry| &entry.descriptor)
    }

    pub fn feature_completions(&self, prefix: &str) -> Vec<CompletionEntry> {
        bounded_completion_entries(
            self.entries
                .values()
                .filter(|entry| entry.handler.is_available())
                .filter(|entry| {
                    entry.descriptor.name.starts_with(prefix)
                        || entry
                            .descriptor
                            .aliases
                            .iter()
                            .any(|alias| alias.starts_with(prefix))
                })
                .map(|entry| CompletionEntry {
                    value: entry.descriptor.name.clone(),
                    is_dir: false,
                    description: Some(entry.descriptor.description.clone()),
                    usage: Some(entry.descriptor.usage()),
                    invocation: Some(entry.descriptor.clone()),
                }),
        )
    }

    pub fn argument_name_completions(
        &self,
        identity: &FeatureInvocationIdentity,
        prefix: &str,
    ) -> Vec<CompletionEntry> {
        bounded_completion_entries(
            self.entries
                .get(identity)
                .filter(|entry| entry.handler.is_available())
                .into_iter()
                .flat_map(|entry| entry.descriptor.arguments.iter())
                .filter(|argument| argument.name.starts_with(prefix))
                .map(|argument| CompletionEntry {
                    value: format!("{}=", argument.name),
                    is_dir: false,
                    description: argument.description.clone(),
                    usage: None,
                    invocation: None,
                }),
        )
    }

    pub async fn argument_completions(
        &self,
        identity: &FeatureInvocationIdentity,
        argument: &str,
        prefix: &str,
    ) -> Vec<CompletionEntry> {
        let Some(entry) = self
            .entries
            .get(identity)
            .filter(|entry| entry.handler.is_available())
        else {
            return Vec::new();
        };
        let Some(descriptor) = entry
            .descriptor
            .arguments
            .iter()
            .find(|candidate| candidate.name == argument)
        else {
            return Vec::new();
        };
        let strings = match &descriptor.completion {
            InvocationCompletion::None => match &descriptor.value_type {
                protocol::InvocationArgumentType::Boolean => ["true", "false"]
                    .into_iter()
                    .filter(|value| value.starts_with(prefix))
                    .map(str::to_owned)
                    .collect(),
                protocol::InvocationArgumentType::Enum { values } => values
                    .iter()
                    .filter(|value| value.starts_with(prefix))
                    .cloned()
                    .collect(),
                _ => Vec::new(),
            },
            InvocationCompletion::WorkerFile | InvocationCompletion::ClientFile => Vec::new(),
            InvocationCompletion::Static { values } => values
                .iter()
                .filter(|value| value.starts_with(prefix))
                .cloned()
                .collect(),
            InvocationCompletion::Provider { .. } => {
                return match &entry.completion_provider {
                    Some(provider) => {
                        let entries = provider
                            .complete(&descriptor.completion, argument, prefix)
                            .await;
                        if !entry.handler.is_available() {
                            return Vec::new();
                        }
                        bounded_completion_entries(entries.into_iter().map(|mut candidate| {
                            // Argument providers contribute values/display text,
                            // never new executable invocation metadata.
                            candidate.invocation = None;
                            candidate
                        }))
                    }
                    None => Vec::new(),
                };
            }
        };
        bounded_completion_entries(strings.into_iter().map(|value| CompletionEntry {
            value,
            is_dir: false,
            description: descriptor.description.clone(),
            usage: None,
            invocation: None,
        }))
    }

    pub(crate) fn validate_metadata(
        &self,
        invocation: &FeatureInvocation,
    ) -> Result<(), FeatureInvocationHandlerError> {
        let entry = self
            .entries
            .get(&invocation.identity)
            .filter(|entry| entry.handler.is_available())
            .ok_or_else(|| {
                FeatureInvocationHandlerError::failed(
                    "Feature invocation is not enabled or currently available",
                )
            })?;
        if entry.descriptor.client_adapter.is_some() {
            return Err(FeatureInvocationHandlerError::failed(
                "client-local invocation must be staged by the client, not executed by the Worker",
            ));
        }
        validate_feature_invocation(&entry.descriptor, invocation).map_err(|error| {
            let mut message = error.to_string();
            truncate_display(&mut message, MAX_INVOCATION_MESSAGE_BYTES);
            FeatureInvocationHandlerError::failed(message)
        })
    }

    pub fn validate(
        &self,
        invocation: &FeatureInvocation,
    ) -> Result<(), FeatureInvocationHandlerError> {
        self.validate_metadata(invocation)?;
        self.entries[&invocation.identity]
            .handler
            .validate(invocation)
            .map_err(|mut error| {
                // Validation is side-effect-free; bound its diagnostic before
                // any start, without claiming an uncertain business outcome.
                truncate_display(&mut error.message, MAX_INVOCATION_MESSAGE_BYTES);
                error
            })
    }

    pub async fn invoke(
        &self,
        invocation: &FeatureInvocation,
    ) -> Result<FeatureInvocationResult, FeatureInvocationHandlerError> {
        self.validate(invocation)?;
        let entry = &self.entries[&invocation.identity];
        let result = entry
            .handler
            .invoke(
                FeatureInvocationContext {
                    invocation_id: invocation.invocation_id.clone(),
                },
                invocation,
            )
            .await
            .map_err(|error| {
                if error.message.len() > MAX_INVOCATION_MESSAGE_BYTES {
                    FeatureInvocationHandlerError::outcome_unknown(
                        "handler error exceeded the result byte limit; do not retry automatically",
                    )
                } else {
                    error
                }
            })?;
        if !invocation_result_is_bounded(&result) {
            return Err(FeatureInvocationHandlerError::outcome_unknown(
                "handler result exceeded the message/context byte limit; do not retry automatically",
            ));
        }
        if result.invocation_id != invocation.invocation_id
            || result.identity != invocation.identity
        {
            return Err(FeatureInvocationHandlerError::outcome_unknown(
                "handler returned a result for a different invocation; do not retry automatically",
            ));
        }
        if result.status != protocol::FeatureInvocationStatus::Succeeded && result.context.is_some()
        {
            return Err(FeatureInvocationHandlerError::outcome_unknown(
                "handler returned preparation context without a successful result",
            ));
        }
        if result.status == protocol::FeatureInvocationStatus::Succeeded
            && !entry.handler.is_available()
        {
            return Err(FeatureInvocationHandlerError::failed(
                "handler reported success but the invocation is no longer available; preparation context withheld; do not retry automatically",
            ));
        }
        Ok(result)
    }

    pub(crate) fn register(
        &mut self,
        feature_id: FeatureId,
        descriptor: FeatureInvocationDescriptor,
        handler: Arc<dyn FeatureInvocationHandler>,
        completion_provider: Option<Arc<dyn FeatureInvocationCompletionProvider>>,
    ) -> Result<(), FeatureInstallError> {
        if descriptor.identity.0.len() > 256
            || serde_json::to_vec(&descriptor).map_or(true, |encoded| {
                encoded.len() > MAX_INVOCATION_DESCRIPTOR_BYTES
            })
        {
            return Err(FeatureInstallError::InvalidDescriptor(
                "invocation metadata exceeds the host byte limit".into(),
            ));
        }
        descriptor
            .validate()
            .map_err(|error| FeatureInstallError::InvalidDescriptor(error.to_string()))?;
        let expected_prefix = format!("{feature_id}/");
        if !descriptor.identity.0.starts_with(&expected_prefix) {
            return Err(FeatureInstallError::InvalidDescriptor(format!(
                "invocation identity `{}` must be qualified by feature `{feature_id}`",
                descriptor.identity
            )));
        }
        if self.entries.contains_key(&descriptor.identity) {
            return Err(FeatureInstallError::DuplicateInvocationIdentity {
                identity: descriptor.identity.to_string(),
            });
        }
        let public_names = std::iter::once(descriptor.name.as_str())
            .chain(descriptor.aliases.iter().map(String::as_str))
            .collect::<BTreeSet<_>>();
        for name in &public_names {
            if let Some(existing) = self.names.get(*name) {
                return Err(FeatureInstallError::DuplicateInvocationName {
                    name: (*name).to_string(),
                    first: existing.to_string(),
                    duplicate: descriptor.identity.to_string(),
                });
            }
        }
        for name in public_names {
            self.names
                .insert(name.to_string(), descriptor.identity.clone());
        }
        self.entries.insert(
            descriptor.identity.clone(),
            RegisteredInvocation {
                descriptor,
                handler,
                completion_provider,
            },
        );
        Ok(())
    }

    pub fn argument_value<'a>(
        invocation: &'a FeatureInvocation,
        name: &str,
    ) -> Option<&'a InvocationValue> {
        invocation
            .arguments
            .iter()
            .find(|argument| argument.name == name)
            .map(|argument| &argument.value)
    }
}

pub struct InvocationContributionRegistrar<'a> {
    pub(super) feature_id: &'a FeatureId,
    pub(super) declarations: &'a [FeatureInvocationDescriptor],
    pub(super) registry: &'a mut FeatureInvocationRegistry,
    pub(super) installed: &'a mut Vec<FeatureInvocationDescriptor>,
}

impl InvocationContributionRegistrar<'_> {
    pub fn register<H>(
        &mut self,
        descriptor: FeatureInvocationDescriptor,
        handler: H,
    ) -> Result<(), FeatureInstallError>
    where
        H: FeatureInvocationHandler + 'static,
    {
        self.register_with_provider(descriptor, Arc::new(handler), None)
    }

    pub fn register_with_completion<H, P>(
        &mut self,
        descriptor: FeatureInvocationDescriptor,
        handler: H,
        provider: P,
    ) -> Result<(), FeatureInstallError>
    where
        H: FeatureInvocationHandler + 'static,
        P: FeatureInvocationCompletionProvider + 'static,
    {
        self.register_with_provider(descriptor, Arc::new(handler), Some(Arc::new(provider)))
    }

    fn register_with_provider(
        &mut self,
        descriptor: FeatureInvocationDescriptor,
        handler: Arc<dyn FeatureInvocationHandler>,
        provider: Option<Arc<dyn FeatureInvocationCompletionProvider>>,
    ) -> Result<(), FeatureInstallError> {
        if provider.is_none()
            && descriptor.arguments.iter().any(|argument| {
                matches!(argument.completion, InvocationCompletion::Provider { .. })
            })
        {
            return Err(FeatureInstallError::InvalidDescriptor(format!(
                "invocation `{}` declares a completion provider but did not register one",
                descriptor.identity
            )));
        }
        if !self.declarations.contains(&descriptor) {
            return Err(FeatureInstallError::UndeclaredContribution {
                kind: super::FeatureContributionKind::ChatInvocation,
                name: descriptor.identity.to_string(),
                feature: self.feature_id.to_string(),
            });
        }
        self.registry.register(
            self.feature_id.clone(),
            descriptor.clone(),
            handler,
            provider,
        )?;
        self.installed.push(descriptor);
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use protocol::{
        FeatureInvocationStatus, FeatureInvocationSyntax, InvocationArgumentDescriptor,
        InvocationArgumentType, InvocationArgumentValue,
    };
    use std::sync::atomic::{AtomicUsize, Ordering};

    fn descriptor(feature: &str, name: &str) -> FeatureInvocationDescriptor {
        FeatureInvocationDescriptor {
            identity: FeatureInvocationIdentity(format!("{feature}/{name}")),
            name: name.into(),
            aliases: Vec::new(),
            display_name: name.into(),
            description: format!("invoke {name}"),
            syntax: FeatureInvocationSyntax::Parenthesized,
            arguments: vec![InvocationArgumentDescriptor {
                name: "value".into(),
                position: Some(0),
                required: true,
                value_type: InvocationArgumentType::String,
                completion: InvocationCompletion::Static {
                    values: vec!["alpha".into(), "beta".into()],
                },
                description: Some("selected value".into()),
            }],
            client_adapter: None,
        }
    }

    struct CountingHandler(Arc<AtomicUsize>);

    #[async_trait]
    impl FeatureInvocationHandler for CountingHandler {
        async fn invoke(
            &self,
            _context: FeatureInvocationContext,
            invocation: &FeatureInvocation,
        ) -> Result<FeatureInvocationResult, FeatureInvocationHandlerError> {
            self.0.fetch_add(1, Ordering::SeqCst);
            Ok(FeatureInvocationResult {
                invocation_id: invocation.invocation_id.clone(),
                identity: invocation.identity.clone(),
                status: FeatureInvocationStatus::Succeeded,
                message: "done".into(),
                context: Some("prepared".into()),
            })
        }
    }

    fn invocation(descriptor: &FeatureInvocationDescriptor) -> FeatureInvocation {
        FeatureInvocation {
            invocation_id: "invoke-1".into(),
            identity: descriptor.identity.clone(),
            name: descriptor.name.clone(),
            arguments: vec![InvocationArgumentValue {
                name: "value".into(),
                value: InvocationValue::String("alpha".into()),
            }],
        }
    }

    #[tokio::test]
    async fn installed_registry_validates_before_calling_handler_and_returns_context() {
        let calls = Arc::new(AtomicUsize::new(0));
        let descriptor = descriptor("builtin:test", "run");
        let mut registry = FeatureInvocationRegistry::default();
        registry
            .register(
                FeatureId::builtin("test"),
                descriptor.clone(),
                Arc::new(CountingHandler(calls.clone())),
                None,
            )
            .unwrap();

        let result = registry.invoke(&invocation(&descriptor)).await.unwrap();
        assert_eq!(result.status, FeatureInvocationStatus::Succeeded);
        assert_eq!(result.context.as_deref(), Some("prepared"));
        assert_eq!(calls.load(Ordering::SeqCst), 1);

        let mut malformed = invocation(&descriptor);
        malformed.arguments.clear();
        assert!(registry.invoke(&malformed).await.is_err());
        assert_eq!(calls.load(Ordering::SeqCst), 1);
    }

    #[test]
    fn registration_rejects_public_name_collisions_across_source_qualified_features() {
        let calls = Arc::new(AtomicUsize::new(0));
        let mut registry = FeatureInvocationRegistry::default();
        registry
            .register(
                FeatureId::builtin("first"),
                descriptor("builtin:first", "run"),
                Arc::new(CountingHandler(calls.clone())),
                None,
            )
            .unwrap();
        let error = registry
            .register(
                FeatureId::builtin("second"),
                descriptor("builtin:second", "run"),
                Arc::new(CountingHandler(calls)),
                None,
            )
            .unwrap_err();
        assert!(matches!(
            error,
            FeatureInstallError::DuplicateInvocationName { .. }
        ));
    }

    #[tokio::test]
    async fn client_local_adapter_metadata_cannot_authorize_worker_execution() {
        let calls = Arc::new(AtomicUsize::new(0));
        let descriptor = crate::feature::builtin::chat_invocation::attach_invocation_descriptor();
        let mut registry = FeatureInvocationRegistry::default();
        registry
            .register(
                FeatureId::builtin("attachments"),
                descriptor.clone(),
                Arc::new(CountingHandler(calls.clone())),
                None,
            )
            .unwrap();
        let invocation = FeatureInvocation {
            invocation_id: "client-path".into(),
            identity: descriptor.identity,
            name: descriptor.name,
            arguments: vec![InvocationArgumentValue {
                name: "file".into(),
                value: InvocationValue::String("/client/private/file".into()),
            }],
        };
        assert!(registry.invoke(&invocation).await.is_err());
        assert_eq!(calls.load(Ordering::SeqCst), 0);
    }

    struct CountingCompletion(Arc<AtomicUsize>);
    #[async_trait]
    impl FeatureInvocationCompletionProvider for CountingCompletion {
        async fn complete(
            &self,
            _completion: &InvocationCompletion,
            _argument: &str,
            _prefix: &str,
        ) -> Vec<CompletionEntry> {
            self.0.fetch_add(1, Ordering::SeqCst);
            Vec::new()
        }
    }

    #[tokio::test]
    async fn completion_dispatch_uses_installed_argument_metadata_not_provider_presence() {
        let calls = Arc::new(AtomicUsize::new(0));
        let provider_calls = Arc::new(AtomicUsize::new(0));
        let descriptor = descriptor("builtin:test", "run");
        let mut registry = FeatureInvocationRegistry::default();
        registry
            .register(
                FeatureId::builtin("test"),
                descriptor.clone(),
                Arc::new(CountingHandler(calls.clone())),
                Some(Arc::new(CountingCompletion(provider_calls.clone()))),
            )
            .unwrap();
        assert_eq!(
            registry
                .argument_completions(&descriptor.identity, "value", "a")
                .await[0]
                .value,
            "alpha"
        );
        assert!(
            registry
                .argument_completions(
                    &FeatureInvocationIdentity("foreign:feature/run".into()),
                    "value",
                    ""
                )
                .await
                .is_empty()
        );
        assert!(
            registry
                .argument_completions(&descriptor.identity, "credential", "")
                .await
                .is_empty()
        );
        assert_eq!(provider_calls.load(Ordering::SeqCst), 0);
        assert_eq!(calls.load(Ordering::SeqCst), 0);
    }

    struct AvailabilityHandler {
        available: Arc<std::sync::atomic::AtomicBool>,
        calls: Arc<AtomicUsize>,
    }
    #[async_trait]
    impl FeatureInvocationHandler for AvailabilityHandler {
        fn is_available(&self) -> bool {
            self.available.load(Ordering::SeqCst)
        }
        async fn invoke(
            &self,
            context: FeatureInvocationContext,
            invocation: &FeatureInvocation,
        ) -> Result<FeatureInvocationResult, FeatureInvocationHandlerError> {
            CountingHandler(self.calls.clone())
                .invoke(context, invocation)
                .await
        }
    }

    struct TestProvider {
        entries: Vec<CompletionEntry>,
        revoke: Option<Arc<std::sync::atomic::AtomicBool>>,
    }
    #[async_trait]
    impl FeatureInvocationCompletionProvider for TestProvider {
        async fn complete(
            &self,
            _completion: &InvocationCompletion,
            _argument: &str,
            _prefix: &str,
        ) -> Vec<CompletionEntry> {
            if let Some(available) = &self.revoke {
                available.store(false, Ordering::SeqCst);
            }
            self.entries.clone()
        }
    }

    #[tokio::test]
    async fn provider_output_is_bounded_and_cannot_inject_invocation_metadata() {
        let available = Arc::new(std::sync::atomic::AtomicBool::new(true));
        let mut descriptor = descriptor("builtin:test", "run");
        descriptor.arguments[0].completion = InvocationCompletion::Provider {
            provider: "choices".into(),
        };
        let entries = (0..128)
            .map(|index| CompletionEntry {
                value: format!("value-{index}"),
                is_dir: false,
                description: Some("é".repeat(MAX_COMPLETION_DESCRIPTION_BYTES)),
                usage: Some("usage".repeat(MAX_COMPLETION_USAGE_BYTES)),
                invocation: Some(descriptor.clone()),
            })
            .chain([CompletionEntry {
                value: "x".repeat(MAX_COMPLETION_VALUE_BYTES + 1),
                is_dir: false,
                description: None,
                usage: None,
                invocation: None,
            }])
            .collect();
        let mut registry = FeatureInvocationRegistry::default();
        registry
            .register(
                FeatureId::builtin("test"),
                descriptor.clone(),
                Arc::new(AvailabilityHandler {
                    available,
                    calls: Arc::new(AtomicUsize::new(0)),
                }),
                Some(Arc::new(TestProvider {
                    entries,
                    revoke: None,
                })),
            )
            .unwrap();
        let candidates = registry
            .argument_completions(&descriptor.identity, "value", "")
            .await;
        assert!(!candidates.is_empty());
        assert!(candidates.len() <= MAX_COMPLETION_ENTRIES);
        assert!(serde_json::to_vec(&candidates).unwrap().len() <= MAX_COMPLETION_RESPONSE_BYTES);
        assert!(candidates.iter().all(|candidate| {
            candidate.value.len() <= MAX_COMPLETION_VALUE_BYTES
                && candidate.invocation.is_none()
                && candidate.description.as_ref().unwrap().len() <= MAX_COMPLETION_DESCRIPTION_BYTES
                && candidate.usage.as_ref().unwrap().len() <= MAX_COMPLETION_USAGE_BYTES
        }));
        let simple = (0..128).map(|index| CompletionEntry {
            value: format!("value-{index}"),
            is_dir: false,
            description: None,
            usage: None,
            invocation: None,
        });
        assert_eq!(
            bounded_completion_entries(simple).len(),
            MAX_COMPLETION_ENTRIES
        );
        assert!(
            bounded_completion_entries([CompletionEntry {
                value: "x".repeat(MAX_COMPLETION_VALUE_BYTES + 1),
                is_dir: false,
                description: None,
                usage: None,
                invocation: None,
            }])
            .is_empty()
        );
    }

    #[tokio::test]
    async fn provider_revocation_during_completion_discards_its_output() {
        let available = Arc::new(std::sync::atomic::AtomicBool::new(true));
        let calls = Arc::new(AtomicUsize::new(0));
        let mut descriptor = descriptor("builtin:test", "run");
        descriptor.arguments[0].completion = InvocationCompletion::Provider {
            provider: "choices".into(),
        };
        let mut registry = FeatureInvocationRegistry::default();
        registry
            .register(
                FeatureId::builtin("test"),
                descriptor.clone(),
                Arc::new(AvailabilityHandler {
                    available: available.clone(),
                    calls: calls.clone(),
                }),
                Some(Arc::new(TestProvider {
                    entries: vec![CompletionEntry {
                        value: "private".into(),
                        is_dir: false,
                        description: None,
                        usage: None,
                        invocation: None,
                    }],
                    revoke: Some(available),
                })),
            )
            .unwrap();
        assert!(
            registry
                .argument_completions(&descriptor.identity, "value", "")
                .await
                .is_empty()
        );
        assert!(registry.descriptors().is_empty());
        assert!(registry.invoke(&invocation(&descriptor)).await.is_err());
        assert_eq!(calls.load(Ordering::SeqCst), 0);
    }

    #[tokio::test]
    async fn enum_and_boolean_completions_derive_from_type_schema_when_no_provider_declared() {
        let calls = Arc::new(AtomicUsize::new(0));
        for (value_type, prefix, expected) in [
            (InvocationArgumentType::Boolean, "f", "false"),
            (
                InvocationArgumentType::Enum {
                    values: vec!["read".into(), "write".into()],
                },
                "w",
                "write",
            ),
        ] {
            let mut descriptor = descriptor("builtin:test", "run");
            descriptor.arguments[0].value_type = value_type;
            descriptor.arguments[0].completion = InvocationCompletion::None;
            let mut registry = FeatureInvocationRegistry::default();
            registry
                .register(
                    FeatureId::builtin("test"),
                    descriptor.clone(),
                    Arc::new(CountingHandler(calls.clone())),
                    None,
                )
                .unwrap();
            let candidates = registry
                .argument_completions(&descriptor.identity, "value", prefix)
                .await;
            assert_eq!(candidates.len(), 1);
            assert_eq!(candidates[0].value, expected);
        }
        assert_eq!(calls.load(Ordering::SeqCst), 0);
    }

    #[test]
    fn oversized_installed_invocation_metadata_is_rejected() {
        let mut descriptor = descriptor("builtin:test", "run");
        descriptor.description = "x".repeat(MAX_INVOCATION_DESCRIPTOR_BYTES);
        let mut registry = FeatureInvocationRegistry::default();
        assert!(
            registry
                .register(
                    FeatureId::builtin("test"),
                    descriptor,
                    Arc::new(CountingHandler(Arc::new(AtomicUsize::new(0)))),
                    None,
                )
                .is_err()
        );
        assert!(registry.descriptors().is_empty());
    }

    #[tokio::test]
    async fn metadata_and_argument_completion_are_side_effect_free() {
        let calls = Arc::new(AtomicUsize::new(0));
        let descriptor = descriptor("builtin:test", "run");
        let mut registry = FeatureInvocationRegistry::default();
        registry
            .register(
                FeatureId::builtin("test"),
                descriptor.clone(),
                Arc::new(CountingHandler(calls.clone())),
                None,
            )
            .unwrap();

        assert_eq!(registry.feature_completions("ru")[0].value, "run");
        assert_eq!(
            registry
                .argument_completions(&descriptor.identity, "value", "a")
                .await[0]
                .value,
            "alpha"
        );
        assert_eq!(calls.load(Ordering::SeqCst), 0);
    }
}
