//! Invocation-local resource identity is independent of owner Ticket selectors.
use super::*;
use server_api::*;

struct OwnerTicket {
    invocation_id: String,
    ticket_id: String,
    writable: bool,
    identity: Arc<()>,
}
fn denied() -> PluginHostError {
    local_error(
        PluginHostErrorCode::Denied,
        PluginEffectObservation::NotDispatched,
    )
}
fn invalid() -> PluginHostError {
    local_error(
        PluginHostErrorCode::InvalidInput,
        PluginEffectObservation::NotDispatched,
    )
}
impl OwnerHost {
    fn ticket<'a>(
        &self,
        binding: &'a (dyn Any + Send + Sync),
    ) -> Result<&'a OwnerTicket, PluginHostError> {
        binding
            .downcast_ref::<OwnerTicket>()
            .filter(|t| {
                t.invocation_id == self.context.invocation_id
                    && Arc::ptr_eq(&t.identity, &self.binding_identity)
            })
            .ok_or_else(denied)
    }
    pub(super) async fn resolve_ticket_binding(
        &self,
        context: &InvocationContext,
        ticket_ref: &str,
        access: objects::Access,
    ) -> Result<Arc<dyn Any + Send + Sync>, PluginHostError> {
        self.check_context(context)?;
        if ticket_ref.is_empty() || ticket_ref.len() > 256 {
            return Err(invalid());
        }
        let writable = matches!(access, objects::Access::ExtensionWrite);
        if if writable {
            context.grants.ticket_data_write.is_empty()
        } else {
            !context.grants.ticket_read && context.grants.ticket_data_read.is_empty()
        } {
            return Err(denied());
        }
        match call_owner(
            self.owner.as_ref(),
            context,
            PluginHostRequest::ResolveTicket {
                invocation_id: context.invocation_id.clone(),
                ticket_ref: ticket_ref.into(),
                access: if writable {
                    PluginTicketAccess::ExtensionWrite
                } else {
                    PluginTicketAccess::Read
                },
            },
        )
        .await?
        {
            PluginHostResponse::TicketResolved { ticket_id }
                if !ticket_id.is_empty() && ticket_id.len() <= 256 =>
            {
                Ok(Arc::new(OwnerTicket {
                    invocation_id: context.invocation_id.clone(),
                    ticket_id,
                    writable,
                    identity: self.binding_identity.clone(),
                }))
            }
            _ => Err(protocol_error(false)),
        }
    }
    pub(super) async fn read_ticket_binding(
        &self,
        context: &InvocationContext,
        binding: &(dyn Any + Send + Sync),
    ) -> Result<ticket_read::Snapshot, PluginHostError> {
        self.check_context(context)?;
        let ticket = self.ticket(binding)?;
        if !context.grants.ticket_read {
            return Err(denied());
        }
        match call_owner(
            self.owner.as_ref(),
            context,
            PluginHostRequest::ReadTicket {
                invocation_id: context.invocation_id.clone(),
                ticket_id: ticket.ticket_id.clone(),
            },
        )
        .await?
        {
            PluginHostResponse::TicketRead { snapshot }
                if snapshot.body.len() <= PLUGIN_TICKET_BODY_LIMIT =>
            {
                Ok(ticket_read::Snapshot {
                    title: snapshot.title,
                    state: match snapshot.case_state {
                        PluginTicketCaseState::Open => ticket_read::CaseState::Open,
                        PluginTicketCaseState::Closed => ticket_read::CaseState::Closed,
                    },
                    content_digest: snapshot.content_digest,
                    body: snapshot.body,
                    truncated: snapshot.truncated,
                })
            }
            _ => Err(protocol_error(false)),
        }
    }
    pub(super) async fn ticket_data_binding(
        &self,
        context: &InvocationContext,
        binding: &(dyn Any + Send + Sync),
        operation: TicketDataOperation,
    ) -> Result<TicketDataResult, PluginHostError> {
        self.check_context(context)?;
        let ticket = self.ticket(binding)?;
        let (collection, write) = match &operation {
            TicketDataOperation::Get(a)
            | TicketDataOperation::LookupMutation { address: a, .. } => (&a.collection, false),
            TicketDataOperation::Mutate { address: a, .. } => (&a.collection, true),
            TicketDataOperation::List { collection, .. } => (collection, false),
        };
        let identity = (
            collection.collection.clone(),
            collection.schema_id.clone(),
            collection.schema_version.clone(),
        );
        let lookup = matches!(&operation, TicketDataOperation::LookupMutation { .. });
        let grants = if write || lookup {
            &context.grants.ticket_data_write
        } else {
            &context.grants.ticket_data_read
        };
        if !grants.contains(&identity) || ((write || lookup) && !ticket.writable) {
            return Err(denied());
        }
        validate_operation(&operation)?;
        let invocation_id = context.invocation_id.clone();
        let ticket_id = ticket.ticket_id.clone();
        let mutation = matches!(&operation, TicketDataOperation::Mutate { .. });
        let request = match operation {
            TicketDataOperation::Get(address) => PluginHostRequest::DataGet {
                invocation_id,
                ticket_id,
                address,
            },
            TicketDataOperation::Mutate {
                address,
                operation_key,
                mutation,
            } => PluginHostRequest::DataMutate {
                invocation_id,
                ticket_id,
                address,
                operation_key,
                mutation,
            },
            TicketDataOperation::List {
                collection,
                cursor,
                limit,
            } => PluginHostRequest::DataList {
                invocation_id,
                ticket_id,
                collection,
                cursor,
                limit,
            },
            TicketDataOperation::LookupMutation {
                address,
                operation_key,
            } => PluginHostRequest::DataLookupMutation {
                invocation_id,
                ticket_id,
                address,
                operation_key,
            },
        };
        match call_owner(self.owner.as_ref(), context, request.clone()).await? {
            PluginHostResponse::DataCell { cell }
                if matches!(request, PluginHostRequest::DataGet { .. }) =>
            {
                Ok(TicketDataResult::Cell(cell))
            }
            PluginHostResponse::DataMutated { receipt } if mutation => {
                Ok(TicketDataResult::Mutated(receipt))
            }
            PluginHostResponse::DataListed { page }
                if matches!(request, PluginHostRequest::DataList { .. }) =>
            {
                Ok(TicketDataResult::Listed(page))
            }
            PluginHostResponse::DataMutationLookup { outcome }
                if matches!(request, PluginHostRequest::DataLookupMutation { .. }) =>
            {
                Ok(TicketDataResult::MutationLookup(outcome))
            }
            _ => Err(protocol_error(mutation)),
        }
    }
}
fn text(s: &str, limit: usize) -> bool {
    !s.is_empty() && s.len() <= limit && !s.chars().any(char::is_control)
}
pub(crate) fn validate_operation(operation: &TicketDataOperation) -> Result<(), PluginHostError> {
    let collection = match operation {
        TicketDataOperation::Get(a)
        | TicketDataOperation::Mutate { address: a, .. }
        | TicketDataOperation::LookupMutation { address: a, .. } => {
            if !text(&a.key, 256) {
                return Err(invalid());
            }
            &a.collection
        }
        TicketDataOperation::List {
            collection,
            cursor,
            limit,
        } => {
            if *limit == 0
                || *limit > PLUGIN_DATA_LIST_LIMIT
                || cursor.as_ref().is_some_and(|c| !text(c, 4096))
            {
                return Err(invalid());
            }
            collection
        }
    };
    if [
        &collection.collection,
        &collection.schema_id,
        &collection.schema_version,
    ]
    .iter()
    .any(|s| !text(s, 128))
    {
        return Err(invalid());
    }
    match operation {
        TicketDataOperation::Mutate {
            operation_key,
            mutation,
            ..
        } => {
            if !text(operation_key, 256) {
                return Err(invalid());
            }
            match mutation {
                PluginDataMutation::Put {
                    expected,
                    value_json,
                } => {
                    if value_json.len() > PLUGIN_DATA_VALUE_LIMIT
                        || serde_json::from_str::<serde_json::Value>(value_json).is_err()
                    {
                        return Err(invalid());
                    }
                    if let PluginDataExpectation::LastWrite(id) = expected {
                        if !text(id, 256) {
                            return Err(invalid());
                        }
                    }
                }
                PluginDataMutation::Delete {
                    expected_last_write,
                } => {
                    if !text(expected_last_write, 256) {
                        return Err(invalid());
                    }
                }
            }
        }
        TicketDataOperation::LookupMutation { operation_key, .. } if !text(operation_key, 256) => {
            return Err(invalid());
        }
        _ => {}
    }
    Ok(())
}
