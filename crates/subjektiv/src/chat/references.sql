-- Presentation references survive reopening and are never reused after erasure.
CREATE TABLE chat_conversation_references (
    number INTEGER PRIMARY KEY AUTOINCREMENT,
    conversation_id TEXT NOT NULL UNIQUE REFERENCES chat_conversations(id) ON DELETE CASCADE
);
INSERT INTO chat_conversation_references(conversation_id)
    SELECT id FROM chat_conversations ORDER BY id;
CREATE TRIGGER chat_conversation_reference_insert AFTER INSERT ON chat_conversations
BEGIN
    INSERT INTO chat_conversation_references(conversation_id) VALUES (NEW.id);
END;
