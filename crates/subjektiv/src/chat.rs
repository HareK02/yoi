//! Public conversation storage only; no delivery queue, Worker ownership or implicit publication.
//!
//! Hosts supply authenticated principals and hold their execution/authorization guard through
//! commit. Membership, exact replay and the user-input fence are checked in the same IMMEDIATE
//! FeatureStorage transaction as append. A saved message proves publication, not Runtime acceptance.
use crate::{SubjektivError, SubjektivStore, require_active_subject};
use feature_storage::FeatureStorageError;
use rusqlite::{Connection, OptionalExtension, Transaction, params};
use server_api::{
    CHAT_DEFAULT_PAGE_LIMIT, CHAT_MAX_COUNTER, CHAT_MAX_PAGE_LIMIT, CHAT_MAX_TEXT_BYTES,
    ChatConversation, ChatCreateConversationRequest, ChatMessage, ChatMessagePage,
    ChatMessagePageQuery, ChatMessageSender, ChatSendMessageRequest, ChatUserMessageRequest,
};
use uuid::Uuid;

pub type Result<T> = std::result::Result<T, ChatError>;

#[derive(Debug, thiserror::Error)]
pub enum ChatError {
    #[error(transparent)]
    Storage(#[from] FeatureStorageError),
    #[error(transparent)]
    Subject(#[from] SubjektivError),
    #[error("conversation not found or not accessible")]
    ConversationNotFound,
    #[error("chat operation `{0}` was reused with different input")]
    OperationConflict(String),
    #[error("chat user input changed: expected {expected:?}, current {actual:?}")]
    UserInputChanged {
        expected: Option<String>,
        actual: Option<String>,
    },
    #[error("chat authorization changed")]
    AuthorizationChanged,
    #[error("invalid chat input: {0}")]
    InvalidInput(String),
}

impl From<rusqlite::Error> for ChatError {
    fn from(error: rusqlite::Error) -> Self {
        Self::Storage(error.into())
    }
}

pub(crate) fn migrate(tx: &Transaction<'_>) -> feature_storage::Result<()> {
    tx.execute_batch(include_str!("chat/schema.sql"))?;
    Ok(())
}

#[derive(Clone, Copy)]
enum Principal<'a> {
    Account(&'a str),
    Subject(&'a str),
}
impl Principal<'_> {
    fn sender(self) -> (&'static str, String) {
        match self {
            Self::Account(id) => ("user", id.into()),
            Self::Subject(id) => ("subject", id.into()),
        }
    }
    fn member_of(self, conversation: &ChatConversation) -> bool {
        match self {
            Self::Account(id) => id == conversation.participant_account_id,
            Self::Subject(id) => id == conversation.subject_id,
        }
    }
}

impl SubjektivStore {
    /// Exact account-scoped replay returns the original creation result (no user input).
    pub fn chat_create_conversation(
        &self,
        authenticated_account_id: &str,
        request: &ChatCreateConversationRequest,
    ) -> Result<ChatConversation> {
        identifier(authenticated_account_id)?;
        identifier(&request.operation_id)?;
        identifier(&request.subject_id)?;
        self.database.try_transaction(|tx| {
            let saved = tx.query_row(
                "SELECT id,subject_id,participant_account_id,last_user_message_id FROM chat_conversations
                 WHERE participant_account_id=?1 AND create_operation_id=?2",
                params![authenticated_account_id, request.operation_id], conversation_row,
            ).optional()?;
            if let Some(mut conversation) = saved {
                if conversation.subject_id != request.subject_id {
                    return Err(ChatError::OperationConflict(request.operation_id.clone()));
                }
                require_active_subject(tx, &conversation.subject_id)?;
                conversation.last_user_message_id = None;
                return Ok(conversation);
            }
            require_active_subject(tx, &request.subject_id)?;
            let conversation = ChatConversation {
                id: Uuid::now_v7().to_string(),
                subject_id: request.subject_id.clone(),
                participant_account_id: authenticated_account_id.into(),
                last_user_message_id: None,
            };
            tx.execute(
                "INSERT INTO chat_conversations
                (id,subject_id,participant_account_id,last_user_message_id,create_operation_id)
                VALUES (?1,?2,?3,NULL,?4)",
                params![
                    conversation.id,
                    conversation.subject_id,
                    authenticated_account_id,
                    request.operation_id
                ],
            )?;
            Ok(conversation)
        })
    }

    pub fn chat_conversation_for_account(
        &self,
        account: &str,
        conversation_id: &str,
    ) -> Result<ChatConversation> {
        self.database
            .try_transaction(|tx| conversation(tx, Principal::Account(account), conversation_id))
    }

    pub fn chat_conversation_for_subject(
        &self,
        subject: &str,
        conversation_id: &str,
    ) -> Result<ChatConversation> {
        self.database
            .try_transaction(|tx| conversation(tx, Principal::Subject(subject), conversation_id))
    }

    pub fn chat_messages_for_account(
        &self,
        account: &str,
        conversation_id: &str,
        query: &ChatMessagePageQuery,
    ) -> Result<ChatMessagePage> {
        self.chat_messages(Principal::Account(account), conversation_id, query, None)
    }

    pub fn chat_messages_for_subject(
        &self,
        subject: &str,
        conversation_id: &str,
        query: &ChatMessagePageQuery,
    ) -> Result<ChatMessagePage> {
        self.chat_messages(Principal::Subject(subject), conversation_id, query, None)
    }

    /// The Host must authorize `chat.read`, then pin that policy's existing change ID.
    /// It must hold current Subject body ownership through this FeatureStorage transaction.
    pub fn chat_messages_for_subject_authorized(
        &self,
        subject: &str,
        conversation_id: &str,
        query: &ChatMessagePageQuery,
        expected_permission_change_id: &str,
    ) -> Result<ChatMessagePage> {
        self.chat_messages(
            Principal::Subject(subject),
            conversation_id,
            query,
            Some(expected_permission_change_id),
        )
    }

    fn chat_messages(
        &self,
        principal: Principal<'_>,
        conversation_id: &str,
        query: &ChatMessagePageQuery,
        expected_permission_change_id: Option<&str>,
    ) -> Result<ChatMessagePage> {
        if query.after_sequence.is_some_and(|s| s > CHAT_MAX_COUNTER) {
            return Err(ChatError::InvalidInput(
                "message cursor exceeds JSON safe-integer range".into(),
            ));
        }
        let limit = query.limit.unwrap_or(CHAT_DEFAULT_PAGE_LIMIT);
        if !(1..=CHAT_MAX_PAGE_LIMIT).contains(&limit) {
            return Err(ChatError::InvalidInput(format!(
                "page limit must be in 1..={CHAT_MAX_PAGE_LIMIT}"
            )));
        }
        self.database.try_transaction(|tx| {
            check_authorization(tx, principal, expected_permission_change_id)?;
            let conversation = conversation(tx, principal, conversation_id)?;
            let after = query.after_sequence.unwrap_or(0) as i64;
            let mut messages = tx.prepare(&format!("SELECT {MESSAGE_COLUMNS} FROM chat_messages WHERE conversation_id=?1 AND sequence>?2 ORDER BY sequence LIMIT ?3"))?
                .query_map(params![conversation_id, after, limit+1], message_row)?
                .collect::<std::result::Result<Vec<_>, _>>()?;
            let has_more = messages.len() > limit as usize;
            messages.truncate(limit as usize);
            Ok(ChatMessagePage { conversation, messages, has_more })
        })
    }

    pub fn chat_append_user_message(
        &self,
        account: &str,
        conversation_id: &str,
        request: &ChatUserMessageRequest,
    ) -> Result<ChatMessage> {
        self.chat_append(
            Principal::Account(account),
            conversation_id,
            Append {
                operation_id: &request.operation_id,
                text: &request.text,
                reply_to: &request.reply_to,
                work_ref: &request.work_ref,
                expected_last_user_message_id: None,
            },
            None,
        )
    }

    /// The authenticated Subject is supplied by the Host, never accepted from the DTO.
    pub fn chat_send_message(
        &self,
        subject: &str,
        request: &ChatSendMessageRequest,
    ) -> Result<ChatMessage> {
        self.chat_send(subject, request, None)
    }

    /// The Host must authorize `chat.send`, then pin that policy's existing change ID.
    /// The ownership guard must span commit; its callback must not reenter the Backend Store.
    pub fn chat_send_message_authorized(
        &self,
        subject: &str,
        request: &ChatSendMessageRequest,
        expected_permission_change_id: &str,
    ) -> Result<ChatMessage> {
        self.chat_send(subject, request, Some(expected_permission_change_id))
    }

    fn chat_send(
        &self,
        subject: &str,
        request: &ChatSendMessageRequest,
        expected_permission_change_id: Option<&str>,
    ) -> Result<ChatMessage> {
        self.chat_append(
            Principal::Subject(subject),
            &request.conversation_id,
            Append {
                operation_id: &request.operation_id,
                text: &request.text,
                reply_to: &request.reply_to,
                work_ref: &request.work_ref,
                expected_last_user_message_id: Some(&request.expected_last_user_message_id),
            },
            expected_permission_change_id,
        )
    }

    fn chat_append(
        &self,
        principal: Principal<'_>,
        conversation_id: &str,
        input: Append<'_>,
        expected_permission_change_id: Option<&str>,
    ) -> Result<ChatMessage> {
        identifier(input.operation_id)?;
        if input.text.trim().is_empty() || input.text.len() > CHAT_MAX_TEXT_BYTES {
            return Err(ChatError::InvalidInput(format!(
                "text must be nonblank and at most {CHAT_MAX_TEXT_BYTES} UTF-8 bytes"
            )));
        }
        for reference in [input.reply_to, input.work_ref]
            .into_iter()
            .flatten()
            .chain(input.expected_last_user_message_id.into_iter().flatten())
        {
            identifier(reference)?;
        }
        self.database.try_transaction(|tx| {
            check_authorization(tx, principal, expected_permission_change_id)?;
            let mut conversation = conversation(tx, principal, conversation_id)?;
            let (kind, sender_id) = principal.sender();
            let saved = tx.query_row(&format!("SELECT {MESSAGE_COLUMNS} FROM chat_messages
                WHERE conversation_id=?1 AND sender_kind=?2 AND sender_id=?3 AND operation_id=?4"),
                params![conversation_id, kind, sender_id, input.operation_id], message_row).optional()?;
            // Replay precedes the current-input fence: ACK loss must remain recoverable after
            // another user input. The pinned observed user identity remains part of the full request.
            if let Some(saved) = saved {
                if saved.text != input.text || &saved.reply_to != input.reply_to || &saved.work_ref != input.work_ref
                    || input.expected_last_user_message_id.is_some_and(|r| r != &saved.last_user_message_id) {
                    return Err(ChatError::OperationConflict(input.operation_id.into()));
                }
                return Ok(saved);
            }
            if let Some(expected) = input.expected_last_user_message_id {
                if expected != &conversation.last_user_message_id {
                    return Err(ChatError::UserInputChanged { expected: expected.clone(), actual: conversation.last_user_message_id });
                }
            }
            if let Some(reply_to) = input.reply_to {
                let exists: bool = tx.query_row("SELECT EXISTS(SELECT 1 FROM chat_messages WHERE conversation_id=?1 AND id=?2)", params![conversation_id,reply_to], |r| r.get(0))?;
                if !exists { return Err(ChatError::InvalidInput("reply_to must reference a message in this conversation".into())); }
            }
            let message_id = Uuid::now_v7().to_string();
            if matches!(principal, Principal::Account(_)) {
                conversation.last_user_message_id = Some(message_id.clone());
                tx.execute("UPDATE chat_conversations SET last_user_message_id=?1 WHERE id=?2", params![message_id,conversation_id])?;
            }
            let last_sequence: u64 = tx.query_row("SELECT COALESCE(MAX(sequence),0) FROM chat_messages WHERE conversation_id=?1", [conversation_id], |r| r.get(0))?;
            let sequence = last_sequence.checked_add(1).filter(|s| *s <= CHAT_MAX_COUNTER)
                .ok_or_else(|| ChatError::InvalidInput("message sequence exhausted".into()))?;
            let message = ChatMessage {
                id: message_id, conversation_id: conversation_id.into(), sequence,
                sender: match principal { Principal::Account(id) => ChatMessageSender::User { account_id: id.into() }, Principal::Subject(id) => ChatMessageSender::Subject { subject_id: id.into() } },
                text: input.text.into(), reply_to: input.reply_to.clone(), work_ref: input.work_ref.clone(),
                operation_id: input.operation_id.into(), last_user_message_id: conversation.last_user_message_id,
            };
            tx.execute("INSERT INTO chat_messages (id,conversation_id,sequence,sender_kind,sender_id,text,reply_to,work_ref,operation_id,last_user_message_id)
                VALUES (?1,?2,?3,?4,?5,?6,?7,?8,?9,?10)", params![message.id,conversation_id,sequence,kind,sender_id,message.text,message.reply_to,message.work_ref,message.operation_id,message.last_user_message_id])?;
            Ok(message)
        })
    }
}

struct Append<'a> {
    operation_id: &'a str,
    text: &'a str,
    reply_to: &'a Option<String>,
    work_ref: &'a Option<String>,
    expected_last_user_message_id: Option<&'a Option<String>>,
}

fn check_authorization(
    tx: &Transaction<'_>,
    principal: Principal<'_>,
    expected: Option<&str>,
) -> Result<()> {
    if let Some(expected) = expected {
        identifier(expected)?;
        let Principal::Subject(subject) = principal else {
            return Err(ChatError::AuthorizationChanged);
        };
        let actual: Option<String> = tx
            .query_row(
                "SELECT change_id FROM subject_permissions WHERE subject_id=?1",
                [subject],
                |row| row.get(0),
            )
            .optional()?;
        if actual.as_deref() != Some(expected) {
            return Err(ChatError::AuthorizationChanged);
        }
    }
    Ok(())
}

fn identifier(value: &str) -> Result<()> {
    if value.trim().is_empty() || value.len() > 512 || value.chars().any(char::is_control) {
        return Err(ChatError::InvalidInput(
            "identifiers must be nonblank, control-free and at most 512 UTF-8 bytes".into(),
        ));
    }
    Ok(())
}

fn conversation(
    tx: &Transaction<'_>,
    principal: Principal<'_>,
    id: &str,
) -> Result<ChatConversation> {
    let conversation = tx.query_row("SELECT id,subject_id,participant_account_id,last_user_message_id FROM chat_conversations WHERE id=?1", [id], conversation_row).optional()?
        .filter(|c| principal.member_of(c)).ok_or(ChatError::ConversationNotFound)?;
    require_active_subject(tx, &conversation.subject_id)?;
    Ok(conversation)
}

fn conversation_row(row: &rusqlite::Row<'_>) -> rusqlite::Result<ChatConversation> {
    Ok(ChatConversation {
        id: row.get(0)?,
        subject_id: row.get(1)?,
        participant_account_id: row.get(2)?,
        last_user_message_id: row.get(3)?,
    })
}

const MESSAGE_COLUMNS: &str = "id,conversation_id,sequence,sender_kind,sender_id,text,reply_to,work_ref,operation_id,last_user_message_id";
fn message_row(row: &rusqlite::Row<'_>) -> rusqlite::Result<ChatMessage> {
    let kind: String = row.get(3)?;
    let id = row.get(4)?;
    let sender = match kind.as_str() {
        "user" => ChatMessageSender::User { account_id: id },
        "subject" => ChatMessageSender::Subject { subject_id: id },
        _ => return Err(rusqlite::Error::InvalidQuery),
    };
    Ok(ChatMessage {
        id: row.get(0)?,
        conversation_id: row.get(1)?,
        sequence: row.get(2)?,
        sender,
        text: row.get(5)?,
        reply_to: row.get(6)?,
        work_ref: row.get(7)?,
        operation_id: row.get(8)?,
        last_user_message_id: row.get(9)?,
    })
}

pub(crate) fn erase_subject(tx: &Connection, subject: &str) -> crate::Result<()> {
    tx.execute("DELETE FROM chat_messages WHERE conversation_id IN (SELECT id FROM chat_conversations WHERE subject_id=?1)", [subject])?;
    tx.execute(
        "DELETE FROM chat_conversations WHERE subject_id=?1",
        [subject],
    )?;
    Ok(())
}

#[cfg(test)]
mod tests;
