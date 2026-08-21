CREATE TABLE message_reactions (
  owner_id TEXT NOT NULL,
  message_id TEXT NOT NULL REFERENCES messages(id) ON DELETE CASCADE,
  emoji TEXT NOT NULL CHECK(length(emoji) BETWEEN 1 AND 64),
  created_at_ms INTEGER NOT NULL,
  PRIMARY KEY(owner_id, message_id, emoji)
);

CREATE INDEX message_reactions_message ON message_reactions(message_id, emoji, owner_id);
