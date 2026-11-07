CREATE TABLE chat_conversations (
    id TEXT PRIMARY KEY,
    subject_id TEXT NOT NULL REFERENCES subjects(subject_id) ON DELETE RESTRICT,
    participant_account_id TEXT NOT NULL,
    last_user_message_id TEXT,
    create_operation_id TEXT NOT NULL,
    UNIQUE(participant_account_id, create_operation_id),
    FOREIGN KEY(id, last_user_message_id) REFERENCES chat_messages(conversation_id, id)
        DEFERRABLE INITIALLY DEFERRED
);

CREATE TABLE chat_messages (
    id TEXT PRIMARY KEY,
    conversation_id TEXT NOT NULL REFERENCES chat_conversations(id) ON DELETE RESTRICT,
    sequence INTEGER NOT NULL CHECK(sequence BETWEEN 1 AND 9007199254740991),
    sender_kind TEXT NOT NULL CHECK(sender_kind IN ('user','subject')),
    sender_id TEXT NOT NULL,
    text TEXT NOT NULL CHECK(length(CAST(text AS BLOB)) BETWEEN 1 AND 16384),
    reply_to TEXT,
    work_ref TEXT,
    operation_id TEXT NOT NULL,
    last_user_message_id TEXT,
    CHECK(sender_kind<>'user' OR (last_user_message_id IS NOT NULL AND last_user_message_id=id)),
    UNIQUE(conversation_id, sequence),
    UNIQUE(conversation_id, id),
    UNIQUE(conversation_id, sender_kind, sender_id, operation_id),
    FOREIGN KEY(conversation_id, reply_to) REFERENCES chat_messages(conversation_id, id)
        DEFERRABLE INITIALLY DEFERRED,
    FOREIGN KEY(conversation_id, last_user_message_id) REFERENCES chat_messages(conversation_id, id)
        DEFERRABLE INITIALLY DEFERRED
);

CREATE TRIGGER chat_conversation_initial_input BEFORE INSERT ON chat_conversations
WHEN NEW.last_user_message_id IS NOT NULL
BEGIN SELECT RAISE(ABORT, 'chat conversations start without user input'); END;

-- The deferred FK requires this fresh identity to be inserted in the same transaction.
-- Membership below requires that row to be user-authored with its own ID as baseline.
CREATE TRIGGER chat_conversation_identity BEFORE UPDATE ON chat_conversations
WHEN OLD.id<>NEW.id OR OLD.subject_id<>NEW.subject_id
    OR OLD.participant_account_id<>NEW.participant_account_id
    OR OLD.create_operation_id<>NEW.create_operation_id
    OR NEW.last_user_message_id IS NULL
    OR NEW.last_user_message_id IS OLD.last_user_message_id
    OR EXISTS(SELECT 1 FROM chat_messages WHERE id=NEW.last_user_message_id)
BEGIN SELECT RAISE(ABORT, 'chat conversation identity or new user identity is invalid'); END;

CREATE TRIGGER chat_messages_immutable BEFORE UPDATE ON chat_messages
BEGIN SELECT RAISE(ABORT, 'chat messages are immutable'); END;

CREATE TRIGGER chat_messages_member BEFORE INSERT ON chat_messages
WHEN NOT EXISTS(SELECT 1 FROM chat_conversations c WHERE c.id=NEW.conversation_id
    AND ((NEW.sender_kind='user' AND c.participant_account_id=NEW.sender_id)
        OR (NEW.sender_kind='subject' AND c.subject_id=NEW.sender_id))
    AND c.last_user_message_id IS NEW.last_user_message_id)
    OR (NEW.sender_kind='subject' AND NEW.last_user_message_id IS NOT NULL
        AND NOT EXISTS(SELECT 1 FROM chat_messages m WHERE m.conversation_id=NEW.conversation_id
            AND m.id=NEW.last_user_message_id AND m.sender_kind='user'))
BEGIN SELECT RAISE(ABORT, 'chat message membership or observed user identity mismatch'); END;

CREATE TRIGGER chat_conversations_deletion_fence BEFORE INSERT ON chat_conversations
WHEN EXISTS(SELECT 1 FROM subject_deletions WHERE subject_id=NEW.subject_id)
BEGIN SELECT RAISE(ABORT, 'Subject deletion has begun'); END;

CREATE TRIGGER chat_conversations_update_fence BEFORE UPDATE ON chat_conversations
WHEN EXISTS(SELECT 1 FROM subject_deletions WHERE subject_id=OLD.subject_id OR subject_id=NEW.subject_id)
BEGIN SELECT RAISE(ABORT, 'Subject deletion has begun'); END;

CREATE TRIGGER chat_messages_deletion_fence BEFORE INSERT ON chat_messages
WHEN EXISTS(SELECT 1 FROM subject_deletions d JOIN chat_conversations c ON c.subject_id=d.subject_id
    WHERE c.id=NEW.conversation_id)
BEGIN SELECT RAISE(ABORT, 'Subject deletion has begun'); END;

CREATE TRIGGER chat_messages_retained BEFORE DELETE ON chat_messages
WHEN NOT EXISTS(SELECT 1 FROM subject_deletions d JOIN chat_conversations c ON c.subject_id=d.subject_id
    WHERE c.id=OLD.conversation_id)
BEGIN SELECT RAISE(ABORT, 'chat messages are retained until Subject erasure'); END;

CREATE TRIGGER chat_conversations_retained BEFORE DELETE ON chat_conversations
WHEN NOT EXISTS(SELECT 1 FROM subject_deletions WHERE subject_id=OLD.subject_id)
BEGIN SELECT RAISE(ABORT, 'chat conversations are retained until Subject erasure'); END;
