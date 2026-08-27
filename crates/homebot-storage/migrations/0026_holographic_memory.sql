CREATE TABLE holographic_facts (
    fact_id INTEGER PRIMARY KEY AUTOINCREMENT,
    owner_id TEXT NOT NULL,
    bot_id TEXT NOT NULL REFERENCES bots(id) ON DELETE CASCADE,
    content TEXT NOT NULL,
    category TEXT NOT NULL DEFAULT 'general'
        CHECK(category IN ('user_pref','project','tool','general')),
    tags TEXT NOT NULL DEFAULT '',
    trust_score REAL NOT NULL DEFAULT 0.5 CHECK(trust_score BETWEEN 0.0 AND 1.0),
    retrieval_count INTEGER NOT NULL DEFAULT 0 CHECK(retrieval_count >= 0),
    helpful_count INTEGER NOT NULL DEFAULT 0 CHECK(helpful_count >= 0),
    source_chat_id TEXT REFERENCES chats(id) ON DELETE SET NULL,
    created_at_ms INTEGER NOT NULL,
    updated_at_ms INTEGER NOT NULL,
    UNIQUE(owner_id, bot_id, content)
);

CREATE INDEX holographic_facts_scope_trust
    ON holographic_facts(owner_id, bot_id, trust_score DESC, updated_at_ms DESC);

CREATE TABLE holographic_entities (
    id TEXT PRIMARY KEY,
    owner_id TEXT NOT NULL,
    bot_id TEXT NOT NULL REFERENCES bots(id) ON DELETE CASCADE,
    name TEXT NOT NULL,
    normalized_name TEXT NOT NULL,
    entity_type TEXT NOT NULL DEFAULT 'unknown',
    aliases TEXT NOT NULL DEFAULT '',
    created_at_ms INTEGER NOT NULL,
    UNIQUE(owner_id, bot_id, normalized_name)
);

CREATE TABLE holographic_fact_entities (
    fact_id INTEGER NOT NULL REFERENCES holographic_facts(fact_id) ON DELETE CASCADE,
    entity_id TEXT NOT NULL REFERENCES holographic_entities(id) ON DELETE CASCADE,
    PRIMARY KEY(fact_id, entity_id)
);

CREATE VIRTUAL TABLE holographic_facts_fts USING fts5(
    fact_id UNINDEXED,
    owner_id UNINDEXED,
    bot_id UNINDEXED,
    content,
    tags,
    tokenize = 'unicode61 remove_diacritics 2',
    prefix = '2 3'
);

CREATE TRIGGER holographic_fact_insert AFTER INSERT ON holographic_facts BEGIN
    INSERT INTO holographic_facts_fts
    VALUES (NEW.fact_id, NEW.owner_id, NEW.bot_id, NEW.content, NEW.tags);
END;

CREATE TRIGGER holographic_fact_update AFTER UPDATE OF content, tags ON holographic_facts BEGIN
    DELETE FROM holographic_facts_fts WHERE fact_id = OLD.fact_id;
    INSERT INTO holographic_facts_fts
    VALUES (NEW.fact_id, NEW.owner_id, NEW.bot_id, NEW.content, NEW.tags);
END;

CREATE TRIGGER holographic_fact_delete AFTER DELETE ON holographic_facts BEGIN
    DELETE FROM holographic_facts_fts WHERE fact_id = OLD.fact_id;
END;
