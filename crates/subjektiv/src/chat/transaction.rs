//! Short native Feature -> Server -> same Feature transaction path.
//!
//! No canonical provider is called under the Host's Server transaction. The Host
//! supplies its current policy there; this guard supplies the unsealed local source
//! otherwise. Commit must occur inside that Host body/policy fence, not afterwards.
use super::*;
use rusqlite::TransactionBehavior;
use server_api::authorization::SubjectPolicySource;

/// Authority location, observed while cutover and legacy writers are fenced.
#[derive(Clone, Debug)]
pub enum ChatPolicySource {
    Local(Option<SubjectPolicySource>),
    Canonical,
}

/// Only chat methods on the already-held native transaction are exposed. Calling
/// SubjektivStore or a canonical provider from the Host callback would reenter locks.
pub struct ChatTransaction<'connection> {
    transaction: Option<Transaction<'connection>>,
    subject: String,
    source: ChatPolicySource,
}

impl ChatTransaction<'_> {
    pub fn policy_source(&self) -> &ChatPolicySource {
        &self.source
    }

    fn transaction(&self) -> Result<&Transaction<'_>> {
        self.transaction
            .as_ref()
            .ok_or(ChatError::AuthorizationChanged)
    }

    pub fn messages(
        &self,
        conversation_id: &str,
        query: &ChatMessagePageQuery,
    ) -> Result<ChatMessagePage> {
        messages_in_transaction(
            self.transaction()?,
            Principal::Subject(&self.subject),
            conversation_id,
            query,
            None,
        )
    }

    pub fn send(&self, request: &ChatSendMessageRequest) -> Result<ChatMessage> {
        append_in_transaction(
            self.transaction()?,
            Principal::Subject(&self.subject),
            &request.conversation_id,
            Append {
                operation_id: &request.operation_id,
                text: &request.text,
                reply_to: &request.reply_to,
                work_ref: &request.work_ref,
                expected_last_user_message_id: Some(&request.expected_last_user_message_id),
            },
            None,
        )
    }

    /// Invoke before leaving the Host's body/current-policy fence. No later chat
    /// operation is possible on this guard. The native connection/lifecycle lease
    /// remains held until the enclosing with_chat_transaction callback returns.
    pub fn commit(&mut self) -> Result<()> {
        self.transaction
            .take()
            .ok_or(ChatError::AuthorizationChanged)?
            .commit()?;
        Ok(())
    }
}

impl SubjektivStore {
    /// Establish FeatureStorage identity/lifecycle, connection and IMMEDIATE writer
    /// fences *before* the Host enters Server SQL (the policy adoption lock order).
    /// Subject lifecycle admission must already be held by the Host. No await or
    /// network/Runtime call is allowed here. Dropping without commit rolls back.
    /// Hosts may preserve their own error type with Ok(host_result) as the result T.
    pub fn with_chat_transaction<T>(
        &self,
        subject: &str,
        action: impl FnOnce(&mut ChatTransaction<'_>) -> Result<T>,
    ) -> Result<T> {
        identifier(subject)?;
        self.database.try_with_connection(|connection| {
            let tx = Transaction::new_unchecked(connection, TransactionBehavior::Immediate)?;
            require_active_subject(&tx, subject)?;
            let source = if self.canonical_permission_source.is_some() {
                ChatPolicySource::Canonical
            } else {
                ChatPolicySource::Local(local_policy_source(&tx, subject)?)
            };
            action(&mut ChatTransaction {
                transaction: Some(tx),
                subject: subject.into(),
                source,
            })
        })
    }
}

pub(super) fn local_policy_source(
    connection: &Connection,
    subject: &str,
) -> Result<Option<SubjectPolicySource>> {
    let raw: Option<String> = connection
        .query_row(
            "SELECT c.record_json FROM subject_permissions p JOIN subject_permission_changes c
         ON c.subject_id=p.subject_id AND c.change_id=p.change_id WHERE p.subject_id=?1",
            [subject],
            |row| row.get(0),
        )
        .optional()?;
    raw.map(|raw| {
        serde_json::from_str(&raw).map_err(|error| ChatError::Subject(SubjektivError::from(error)))
    })
    .transpose()
}
