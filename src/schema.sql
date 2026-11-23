CREATE TABLE IF NOT EXISTS threads (
  id INTEGER PRIMARY KEY, channel TEXT NOT NULL, title TEXT NOT NULL, label TEXT NOT NULL,
  dependency INTEGER REFERENCES threads(id), owner INTEGER, expires INTEGER, done INTEGER DEFAULT 0
);
CREATE TABLE IF NOT EXISTS messages (
  id INTEGER PRIMARY KEY, thread INTEGER NOT NULL REFERENCES threads(id), actor INTEGER NOT NULL,
  sender_email TEXT, body TEXT NOT NULL, created INTEGER NOT NULL DEFAULT (unixepoch())
);
CREATE INDEX IF NOT EXISTS message_thread ON messages(thread,id);
CREATE TABLE IF NOT EXISTS channels (id INTEGER PRIMARY KEY, name TEXT UNIQUE NOT NULL);
INSERT OR IGNORE INTO channels(name) SELECT DISTINCT channel FROM threads;
INSERT OR IGNORE INTO channels(name) VALUES ('general');
CREATE VIEW IF NOT EXISTS overview AS
SELECT t.id,t.channel,t.title,t.label,t.dependency,
  CASE WHEN t.done OR t.expires>unixepoch() THEN t.owner END AS owner,
  CASE WHEN t.done THEN 'done' WHEN parent.done=0 THEN 'blocked'
       WHEN t.expires>unixepoch() THEN 'claimed' ELSE 'ready' END AS status
FROM threads t LEFT JOIN threads parent ON t.dependency=parent.id;
CREATE TABLE IF NOT EXISTS pending_message_events (
  message_id INTEGER PRIMARY KEY REFERENCES messages(id),
  event_id TEXT NOT NULL UNIQUE DEFAULT (lower(hex(randomblob(16)))),
  occurred_at_ms INTEGER NOT NULL DEFAULT (CAST(unixepoch('subsec') * 1000 AS INTEGER)),
  origin_chat_id TEXT
);

CREATE TABLE IF NOT EXISTS message_agents (
  message_id INTEGER PRIMARY KEY REFERENCES messages(id) ON DELETE CASCADE,
  chat_id TEXT NOT NULL,
  name TEXT
);
INSERT OR IGNORE INTO message_agents(message_id, chat_id)
SELECT message_id, origin_chat_id FROM pending_message_events WHERE origin_chat_id IS NOT NULL;
