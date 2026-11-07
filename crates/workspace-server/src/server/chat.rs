//! Public messages and Runtime notification receipts have separate owners.
//!
//! A receive first commits the account-bound message, then makes one ordinary Notify
//! attempt against the existing Subject singleton. No scheduler, mailbox, implicit
//! restore or replacement body is created. Explicit same-operation retry recovers the
//! saved message and reuses its Notify ID; receipts have the existing bounded retention.
//! Subject publication holds the existing lifecycle gate and rechecks live ownership
//! and operation permission before the FeatureStorage observed-user-message transaction.
use super::*;
use server_api::*;

#[cfg(test)]
static ADMISSION_OBSERVERS: std::sync::LazyLock<
    std::sync::Mutex<std::collections::HashMap<String, std::sync::mpsc::SyncSender<()>>>,
> = std::sync::LazyLock::new(Default::default);

#[cfg(test)]
pub(super) fn observe_next_admission(subject: &str) -> std::sync::mpsc::Receiver<()> {
    let (send, receive) = std::sync::mpsc::sync_channel(1);
    assert!(
        ADMISSION_OBSERVERS
            .lock()
            .unwrap()
            .insert(subject.into(), send)
            .is_none()
    );
    receive
}

fn domain(error: crate::subjektiv::chat::ChatError) -> ApiError {
    use crate::subjektiv::chat::ChatError;
    match error {
        ChatError::AuthorizationChanged => Error::WorkspacePermissionDenied(error.to_string()),
        ChatError::ConversationNotFound => Error::WorkspacePermissionDenied(error.to_string()),
        ChatError::OperationConflict(_) | ChatError::UserInputChanged { .. } => {
            Error::RepositoryConflict(error.to_string())
        }
        ChatError::Storage(_) => Error::Store(error.to_string()),
        ChatError::Subject(error) => return subjektiv_store_error(error).into(),
        ChatError::InvalidInput(_) => Error::InvalidInput(error.to_string()),
    }
    .into()
}

async fn account_context(
    api: &WorkspaceApi,
    context: &ServerRequestContext,
    workspace: &str,
) -> ApiResult<(String, WorkerOperationContext)> {
    let (account, authority) = read_context(api, context, workspace).await?;
    authority.authorize_workspace(api).await?;
    Ok((account, authority))
}

async fn read_context(
    api: &WorkspaceApi,
    context: &ServerRequestContext,
    workspace: &str,
) -> ApiResult<(String, WorkerOperationContext)> {
    if context.runtime_source.is_some() || context.worker_source.is_some() {
        return Err(Error::WorkspacePermissionDenied(
            "chat user input requires an authenticated Account, not a Worker".into(),
        )
        .into());
    }
    let actor = context.actor.as_ref().ok_or_else(|| {
        Error::WorkspacePermissionDenied("chat requires an authenticated Account".into())
    })?;
    require_workspace_owner(api, workspace, actor, "chat account access").await?;
    let authority = WorkerOperationContext::from_request_async(api, context)
        .await
        .map_err(|error| Error::WorkspacePermissionDenied(error.message))?;
    authority.authorize_workspace_read(api).await?;
    Ok((actor.account_id.clone(), authority))
}

pub(super) async fn create(
    api: WorkspaceApi,
    context: ServerRequestContext,
    workspace: String,
    request: ChatCreateConversationRequest,
) -> ApiResult<ChatConversation> {
    let (account, _) = account_context(&api, &context, &workspace).await?;
    api.store
        .db_executor()
        .run(move || -> ApiResult<_> {
            open_subjektiv_store(&api)?
                .chat_create_conversation(&account, &request)
                .map_err(domain)
        })
        .await
}

pub(super) async fn messages(
    api: WorkspaceApi,
    context: ServerRequestContext,
    workspace: String,
    conversation: String,
    query: ChatMessagePageQuery,
) -> ApiResult<ChatMessagePage> {
    let (account, _) = read_context(&api, &context, &workspace).await?;
    api.store
        .db_executor()
        .run(move || -> ApiResult<_> {
            open_subjektiv_store(&api)?
                .chat_messages_for_account(&account, &conversation, &query)
                .map_err(domain)
        })
        .await
}

pub(super) async fn receive(
    api: WorkspaceApi,
    context: ServerRequestContext,
    workspace: String,
    conversation: String,
    request: ChatUserMessageRequest,
) -> ApiResult<ChatReceiveResponse> {
    let (account, authority) = account_context(&api, &context, &workspace).await?;
    let save_api = api.clone();
    let (message, subject, reference) = api
        .store
        .db_executor()
        .run(move || -> ApiResult<_> {
            let store = open_subjektiv_store(&save_api)?;
            let message = store
                .chat_append_user_message(&account, &conversation, &request)
                .map_err(domain)?;
            let conversation = store
                .chat_conversation_for_account(&account, &conversation)
                .map_err(domain)?;
            let reference = store
                .chat_conversation_reference(&conversation.subject_id, &conversation.id)
                .map_err(domain)?;
            Ok((message, conversation.subject_id, reference))
        })
        .await?;
    // Delivery identity and authenticated provenance stay in the transport envelope.
    // The model needs only a resolvable conversation and the public message content.
    #[derive(serde::Serialize)]
    struct MessageNotice<'a> {
        conversation: &'a str,
        sender: &'static str,
        text: &'a str,
    }
    let content = serde_yaml::to_string(&MessageNotice {
        conversation: &reference,
        sender: "user",
        text: &message.text,
    })
    .map_err(|e| Error::Store(e.to_string()))?;
    let resolve_api = api.clone();
    let delivery_subject = subject.clone();
    let target = api
        .store
        .db_executor()
        .run(move || -> ApiResult<_> {
            let key = crate::subjektiv::subject_worker_singleton_key(&subject)
                .map_err(|e| Error::InvalidInput(e.to_string()))?;
            Ok(resolve_api
                .store
                .current_worker_singleton_owner(&workspace, &key)?
                .map(|lease| lease.worker))
        })
        .await?;
    let delivery = if let Some(target) = target {
        let attempt = async {
            let worker =
                WorkspaceWorker::resolve_async(&api, &target.runtime_id, &target.worker_id).await?;
            worker
                .input_current_subject(
                    &authority,
                    WorkerInputRequest {
                        kind: WorkerInputKind::Notify,
                        content,
                        submission_request_id: Some(format!("chat-message:{}", message.id)),
                        segments: None,
                        source: None,
                    },
                    &delivery_subject,
                )
                .await
        }
        .await;
        match attempt {
            Ok(result) => {
                let detail = result
                    .diagnostics
                    .iter()
                    .map(|d| d.message.as_str())
                    .collect::<Vec<_>>()
                    .join("; ");
                match result.disposition {
                    WorkerInputDisposition::Accepted => ChatDelivery::Accepted {
                        runtime_id: target.runtime_id,
                        worker_id: target.worker_id,
                    },
                    WorkerInputDisposition::Rejected => ChatDelivery::Rejected {
                        runtime_id: target.runtime_id,
                        worker_id: target.worker_id,
                        detail,
                    },
                    WorkerInputDisposition::Unknown => ChatDelivery::Unknown {
                        runtime_id: target.runtime_id,
                        worker_id: target.worker_id,
                        detail,
                    },
                }
            }
            // Transport/Host failures can occur after dispatch; absence of an ACK is
            // not proof of rejection. Same request recovery is safe within receipt retention.
            Err(error) => ChatDelivery::Unknown {
                runtime_id: target.runtime_id,
                worker_id: target.worker_id,
                detail: error.error.to_string(),
            },
        }
    } else {
        ChatDelivery::NoCurrentBody
    };
    Ok(ChatReceiveResponse { message, delivery })
}

fn authorize_chat_policy(
    registry: &worker::feature::permissions::FeaturePermissionRegistry,
    workspace: &str,
    subject: &str,
    operation: &str,
    policy: &server_api::authorization::SubjectPolicySource,
) -> ApiResult<()> {
    use ::feature_permissions::PermissionKey;
    use server_api::authorization::{PermissionResource, ScopedPermission};
    let denied = || {
        Error::WorkspacePermissionDenied(format!(
            "current Subject policy does not permit {operation}"
        ))
    };
    if registry.host().catalog().workspace.as_str() != workspace {
        return Err(denied().into());
    }
    let key = PermissionKey::parse(operation).map_err(|_| denied())?;
    // The installed callee owns chat's namespace. Caller claims and textual
    // namespace prefixes cannot select a different Feature's handle.
    let feature = worker::feature::FeatureId::builtin("subjektiv-chat");
    let namespace = registry
        .namespace_handle(&feature, &key.namespace())
        .ok_or_else(denied)?;
    // Scalar legacy activation entries are retained as domain constraints; they
    // neither fail unrelated chat access nor imply dotted operations.
    let exact = ::feature_permissions::PermissionAssignment {
        grants: vec![::feature_permissions::PermissionReference::Namespace(
            ::feature_permissions::NamespacePermissions {
                namespace: key.namespace(),
                keys: vec![key.clone()],
            },
        )],
        ..Default::default()
    };
    let (assignment, legacy) = match policy {
        server_api::authorization::SubjectPolicySource::Feature(policy) => {
            (&policy.execution, policy.legacy_constraints.as_ref())
        }
        server_api::authorization::SubjectPolicySource::Legacy(policy) => (&exact, Some(policy)),
    };
    let resolved = registry.host().resolve(Some(assignment));
    if !registry
        .host()
        .allows(namespace, &resolved, &key)
        .map_err(|_| denied())?
    {
        return Err(denied().into());
    }
    let needed = ScopedPermission {
        operation: operation.into(),
        resource: PermissionResource::Subject {
            workspace_id: workspace.into(),
            subject_id: subject.into(),
        },
    };
    if let Some(legacy) = legacy {
        if !legacy.execution.iter().any(|grant| grant.covers(&needed)) {
            return Err(denied().into());
        }
    }
    Ok(())
}

pub(super) async fn worker_operation(
    api: WorkspaceApi,
    context: ServerRequestContext,
    workspace: String,
    request: ChatWorkerRequest,
) -> ApiResult<ChatWorkerResponse> {
    worker_operation_inner(
        api,
        context,
        workspace,
        request,
        #[cfg(test)]
        None,
        #[cfg(test)]
        None,
    )
    .await
}

// Explicitly scoped to this invocation: parallel tests and other Workspace APIs
// cannot intercept an unrelated request's native admission.
#[cfg(test)]
pub(super) async fn worker_operation_before_feature(
    api: WorkspaceApi,
    context: ServerRequestContext,
    workspace: String,
    request: ChatWorkerRequest,
    observe: Arc<dyn Fn() + Send + Sync>,
) -> ApiResult<ChatWorkerResponse> {
    worker_operation_inner(api, context, workspace, request, Some(observe), None).await
}

#[cfg(test)]
pub(super) async fn worker_operation_before_host(
    api: WorkspaceApi,
    context: ServerRequestContext,
    workspace: String,
    request: ChatWorkerRequest,
    observe: Arc<dyn Fn() + Send + Sync>,
) -> ApiResult<ChatWorkerResponse> {
    worker_operation_inner(api, context, workspace, request, None, Some(observe)).await
}

async fn worker_operation_inner(
    api: WorkspaceApi,
    context: ServerRequestContext,
    workspace: String,
    request: ChatWorkerRequest,
    #[cfg(test)] before_feature: Option<Arc<dyn Fn() + Send + Sync>>,
    #[cfg(test)] before_host: Option<Arc<dyn Fn() + Send + Sync>>,
) -> ApiResult<ChatWorkerResponse> {
    api.host_executors
        .feature()?
        .run(move || -> ApiResult<_> {
            validate_workspace_scope(&api, &workspace)?;
            let initial = subjektiv_worker_scope_from_store(&api, &workspace, &context)?;
            let request_source = context.runtime_source.as_ref().ok_or_else(|| {
                Error::WorkspacePermissionDenied("chat requires a signed Runtime source".into())
            })?;
            api.config_store.with_conn(|connection| {
                crate::SqliteWorkspaceStore::require_authenticated_subject_request(
                    connection,
                    &workspace,
                    request_source,
                    &initial.worker,
                )
            })?;
            if !matches!(initial.authority, SubjektivStoredScopeAuthority::Subject) {
                return Err(Error::WorkspacePermissionDenied(
                    "chat publication and reads require the current Subject body".into(),
                )
                .into());
            }
            let gate = subjektiv_journal::lifecycle_lock(&api, &initial.subject_id)?;
            #[cfg(test)]
            if let Some(observer) = ADMISSION_OBSERVERS
                .lock()
                .unwrap()
                .remove(&initial.subject_id)
            {
                let _ = observer.send(());
            }
            let _gate = gate
                .lock()
                .map_err(|_| Error::Store("Subject lifecycle lock poisoned".into()))?;
            let scope = subjektiv_worker_scope_from_store(&api, &workspace, &context)?;
            if scope.subject_id != initial.subject_id
                || !matches!(scope.authority, SubjektivStoredScopeAuthority::Subject)
            {
                return Err(Error::WorkspacePermissionDenied(
                    "chat Subject body binding changed while waiting for admission".into(),
                )
                .into());
            }
            let operation = match &request.operation {
                ChatWorkerOperation::Read { .. } => "chat.read",
                ChatWorkerOperation::Send { .. } => "chat.send",
            };
            let store = open_subjektiv_store(&api)?;
            let lease = api
                .store
                .require_current_worker_singleton_owner(&workspace, &scope.worker)?
                .ok_or_else(|| {
                    Error::WorkspacePermissionDenied("Subject body ownership is absent".into())
                })?;
            #[cfg(test)]
            if let Some(observe) = before_feature {
                observe();
            }
            // Match native Memory admission: Feature DB -> registry -> Host.
            // Never pin the registry while waiting for the Feature connection;
            // Memory already holds that connection when it requests registry write.
            store
                .with_chat_transaction(&scope.subject_id, |chat| {
                    #[cfg(test)]
                    if let Some(observe) = before_host {
                        observe();
                    }
                    Ok((|| -> ApiResult<_> {
                        let registry = api.permission_registry.read().map_err(|_| {
                            Error::WorkspacePermissionDenied(
                                "current Feature declarations unavailable".into(),
                            )
                        })?;
                        let source = chat.policy_source().clone();
                        api.config_store
                            .with_current_subject_body_policy(
                                &workspace,
                                &scope.subject_id,
                                &lease,
                                &source,
                                request_source,
                                |policy| {
                                    Ok((|| -> ApiResult<_> {
                                        authorize_chat_policy(
                                            &registry,
                                            &workspace,
                                            &scope.subject_id,
                                            operation,
                                            policy,
                                        )?;
                                        let response = match &request.operation {
                                            ChatWorkerOperation::Read {
                                                conversation_id,
                                                query,
                                            } => chat
                                                .messages(conversation_id, query)
                                                .map(|page| ChatWorkerResponse::Messages { page }),
                                            ChatWorkerOperation::Send { request } => {
                                                chat.send(request).map(|message| {
                                                    ChatWorkerResponse::Sent { message }
                                                })
                                            }
                                        }
                                        .map_err(domain)?;
                                        // Both policy and ownership stay fenced through commit;
                                        // replay is authorized before any saved result is read.
                                        chat.commit().map_err(domain)?;
                                        Ok(response)
                                    })())
                                },
                            )
                            .map_err(ApiError::from)?
                    })())
                })
                .map_err(domain)?
        })
        .await
}
