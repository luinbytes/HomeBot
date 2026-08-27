CREATE VIRTUAL TABLE search_documents USING fts5(
    owner_id UNINDEXED,
    kind UNINDEXED,
    source_id UNINDEXED,
    chat_id UNINDEXED,
    message_id UNINDEXED,
    artifact_id UNINDEXED,
    routine_id UNINDEXED,
    title,
    body,
    created_at_ms UNINDEXED,
    tokenize = 'unicode61 remove_diacritics 2',
    prefix = '2 3'
);

INSERT INTO search_documents
SELECT c.owner_id, 'message', p.id, m.chat_id, m.id, '', '', 'Message',
       CAST(json_extract(p.content_json, '$.text') AS TEXT), m.created_at_ms
FROM message_parts p
JOIN messages m ON m.id = p.message_id
JOIN chats c ON c.id = m.chat_id
WHERE p.kind IN ('text', 'notice');

INSERT INTO search_documents
SELECT owner_id, 'file', id, chat_id, coalesce(message_id, ''), id, '', name,
       kind || ' ' || media_type, created_at_ms
FROM artifacts;

INSERT INTO search_documents
SELECT c.owner_id, 'file', p.id, m.chat_id, m.id, '', '', a.filename,
       a.media_type, m.created_at_ms
FROM message_parts p
JOIN messages m ON m.id = p.message_id
JOIN chats c ON c.id = m.chat_id
JOIN attachments a ON a.id = json_extract(p.content_json, '$.attachment_id')
WHERE p.kind = 'attachment' AND a.status = 'ready';

INSERT INTO search_documents
SELECT owner_id, 'routine', id, '', '', '', id, name, description, updated_at_ms
FROM routines WHERE bot_id IS NOT NULL;

CREATE TRIGGER search_message_part_insert AFTER INSERT ON message_parts
WHEN NEW.kind IN ('text', 'notice')
BEGIN
    INSERT INTO search_documents
    SELECT c.owner_id, 'message', NEW.id, m.chat_id, m.id, '', '', 'Message',
           CAST(json_extract(NEW.content_json, '$.text') AS TEXT), m.created_at_ms
    FROM messages m JOIN chats c ON c.id = m.chat_id WHERE m.id = NEW.message_id;
END;

CREATE TRIGGER search_attachment_part_insert AFTER INSERT ON message_parts
WHEN NEW.kind = 'attachment'
BEGIN
    INSERT INTO search_documents
    SELECT c.owner_id, 'file', NEW.id, m.chat_id, m.id, '', '', a.filename,
           a.media_type, m.created_at_ms
    FROM messages m
    JOIN chats c ON c.id = m.chat_id
    JOIN attachments a ON a.id = json_extract(NEW.content_json, '$.attachment_id')
    WHERE m.id = NEW.message_id AND a.status = 'ready';
END;

CREATE TRIGGER search_message_part_update AFTER UPDATE ON message_parts
BEGIN
    DELETE FROM search_documents WHERE source_id = OLD.id AND kind IN ('message', 'file');
    INSERT INTO search_documents
    SELECT c.owner_id, 'message', NEW.id, m.chat_id, m.id, '', '', 'Message',
           CAST(json_extract(NEW.content_json, '$.text') AS TEXT), m.created_at_ms
    FROM messages m JOIN chats c ON c.id = m.chat_id
    WHERE m.id = NEW.message_id AND NEW.kind IN ('text', 'notice');
    INSERT INTO search_documents
    SELECT c.owner_id, 'file', NEW.id, m.chat_id, m.id, '', '', a.filename,
           a.media_type, m.created_at_ms
    FROM messages m
    JOIN chats c ON c.id = m.chat_id
    JOIN attachments a ON a.id = json_extract(NEW.content_json, '$.attachment_id')
    WHERE m.id = NEW.message_id AND NEW.kind = 'attachment' AND a.status = 'ready';
END;

CREATE TRIGGER search_message_part_delete AFTER DELETE ON message_parts
BEGIN
    DELETE FROM search_documents WHERE source_id = OLD.id AND kind IN ('message', 'file');
END;

CREATE TRIGGER search_artifact_insert AFTER INSERT ON artifacts
BEGIN
    INSERT INTO search_documents VALUES (
        NEW.owner_id, 'file', NEW.id, NEW.chat_id, coalesce(NEW.message_id, ''), NEW.id, '',
        NEW.name, NEW.kind || ' ' || NEW.media_type, NEW.created_at_ms
    );
END;

CREATE TRIGGER search_artifact_update AFTER UPDATE ON artifacts
BEGIN
    DELETE FROM search_documents WHERE kind = 'file' AND source_id = OLD.id AND artifact_id = OLD.id;
    INSERT INTO search_documents VALUES (
        NEW.owner_id, 'file', NEW.id, NEW.chat_id, coalesce(NEW.message_id, ''), NEW.id, '',
        NEW.name, NEW.kind || ' ' || NEW.media_type, NEW.created_at_ms
    );
END;

CREATE TRIGGER search_artifact_delete AFTER DELETE ON artifacts
BEGIN
    DELETE FROM search_documents WHERE kind = 'file' AND source_id = OLD.id AND artifact_id = OLD.id;
END;

CREATE TRIGGER search_attachment_ready AFTER UPDATE OF status ON attachments
WHEN OLD.status != 'ready' AND NEW.status = 'ready'
BEGIN
    INSERT INTO search_documents
    SELECT c.owner_id, 'file', p.id, m.chat_id, m.id, '', '', NEW.filename,
           NEW.media_type, m.created_at_ms
    FROM message_parts p
    JOIN messages m ON m.id = p.message_id
    JOIN chats c ON c.id = m.chat_id
    WHERE p.kind = 'attachment' AND json_extract(p.content_json, '$.attachment_id') = NEW.id;
END;

CREATE TRIGGER search_attachment_update AFTER UPDATE OF filename, media_type ON attachments
WHEN NEW.status = 'ready'
BEGIN
    DELETE FROM search_documents
    WHERE kind = 'file' AND source_id IN (
        SELECT id FROM message_parts
        WHERE kind = 'attachment' AND json_extract(content_json, '$.attachment_id') = OLD.id
    );
    INSERT INTO search_documents
    SELECT c.owner_id, 'file', p.id, m.chat_id, m.id, '', '', NEW.filename,
           NEW.media_type, m.created_at_ms
    FROM message_parts p
    JOIN messages m ON m.id = p.message_id
    JOIN chats c ON c.id = m.chat_id
    WHERE p.kind = 'attachment' AND json_extract(p.content_json, '$.attachment_id') = NEW.id;
END;

CREATE TRIGGER search_attachment_delete AFTER DELETE ON attachments
BEGIN
    DELETE FROM search_documents
    WHERE kind = 'file' AND source_id IN (
        SELECT id FROM message_parts
        WHERE kind = 'attachment' AND json_extract(content_json, '$.attachment_id') = OLD.id
    );
END;

CREATE TRIGGER search_routine_insert AFTER INSERT ON routines
WHEN NEW.bot_id IS NOT NULL
BEGIN
    INSERT INTO search_documents VALUES (
        NEW.owner_id, 'routine', NEW.id, '', '', '', NEW.id,
        NEW.name, NEW.description, NEW.updated_at_ms
    );
END;

CREATE TRIGGER search_routine_update AFTER UPDATE ON routines
BEGIN
    DELETE FROM search_documents WHERE kind = 'routine' AND source_id = OLD.id;
    INSERT INTO search_documents
    SELECT NEW.owner_id, 'routine', NEW.id, '', '', '', NEW.id,
           NEW.name, NEW.description, NEW.updated_at_ms
    WHERE NEW.bot_id IS NOT NULL;
END;

CREATE TRIGGER search_routine_delete AFTER DELETE ON routines
BEGIN
    DELETE FROM search_documents WHERE kind = 'routine' AND source_id = OLD.id;
END;
