-- Images (game art, avatars) are stored content-addressed on disk; these columns hold image ids.
ALTER TABLE games ADD COLUMN cover_image TEXT;
ALTER TABLE games ADD COLUMN banner_image TEXT;
ALTER TABLE users ADD COLUMN avatar_image TEXT;
ALTER TABLE users ADD COLUMN last_seen_at INTEGER NOT NULL DEFAULT 0;

-- One row per pair of users, stored with user_a < user_b.
CREATE TABLE friendships (
    user_a       INTEGER NOT NULL REFERENCES users(id) ON DELETE CASCADE,
    user_b       INTEGER NOT NULL REFERENCES users(id) ON DELETE CASCADE,
    requested_by INTEGER NOT NULL REFERENCES users(id) ON DELETE CASCADE,
    status       TEXT    NOT NULL CHECK (status IN ('pending', 'accepted')),
    created_at   INTEGER NOT NULL,
    accepted_at  INTEGER,
    PRIMARY KEY (user_a, user_b),
    CHECK (user_a < user_b)
);
CREATE INDEX friendships_b ON friendships(user_b);

CREATE TABLE messages (
    id           INTEGER PRIMARY KEY,
    sender_id    INTEGER NOT NULL REFERENCES users(id) ON DELETE CASCADE,
    recipient_id INTEGER NOT NULL REFERENCES users(id) ON DELETE CASCADE,
    body         TEXT    NOT NULL,
    created_at   INTEGER NOT NULL,
    read_at      INTEGER
);
CREATE INDEX messages_pair ON messages(sender_id, recipient_id, id);
CREATE INDEX messages_unread ON messages(recipient_id, read_at);

CREATE TABLE playtime (
    user_id        INTEGER NOT NULL REFERENCES users(id) ON DELETE CASCADE,
    game_id        INTEGER NOT NULL REFERENCES games(id) ON DELETE CASCADE,
    seconds        INTEGER NOT NULL DEFAULT 0,
    last_played_at INTEGER,
    PRIMARY KEY (user_id, game_id)
);
