CREATE TABLE users (
    id            INTEGER PRIMARY KEY,
    username      TEXT    NOT NULL UNIQUE COLLATE NOCASE,
    display_name  TEXT    NOT NULL,
    password_hash TEXT    NOT NULL,
    bio           TEXT    NOT NULL DEFAULT '',
    is_admin      INTEGER NOT NULL DEFAULT 0,
    created_at    INTEGER NOT NULL
);

-- Login sessions and publish tokens. Only a hash of each token is stored.
CREATE TABLE tokens (
    id           INTEGER PRIMARY KEY,
    user_id      INTEGER NOT NULL REFERENCES users(id) ON DELETE CASCADE,
    kind         TEXT    NOT NULL CHECK (kind IN ('session', 'publish')),
    name         TEXT    NOT NULL DEFAULT '',
    token_hash   TEXT    NOT NULL UNIQUE,
    game_id      INTEGER REFERENCES games(id) ON DELETE CASCADE,
    created_at   INTEGER NOT NULL,
    last_used_at INTEGER NOT NULL,
    expires_at   INTEGER
);
CREATE INDEX tokens_user ON tokens(user_id);

CREATE TABLE invites (
    code       TEXT    PRIMARY KEY,
    created_by INTEGER REFERENCES users(id) ON DELETE SET NULL,
    used_by    INTEGER REFERENCES users(id) ON DELETE SET NULL,
    created_at INTEGER NOT NULL,
    used_at    INTEGER
);

CREATE TABLE games (
    id          INTEGER PRIMARY KEY,
    slug        TEXT    NOT NULL UNIQUE,
    title       TEXT    NOT NULL,
    owner_id    INTEGER NOT NULL REFERENCES users(id),
    summary     TEXT    NOT NULL DEFAULT '',
    description TEXT    NOT NULL DEFAULT '',
    visibility  TEXT    NOT NULL DEFAULT 'public' CHECK (visibility IN ('public', 'unlisted', 'private')),
    created_at  INTEGER NOT NULL,
    updated_at  INTEGER NOT NULL
);

CREATE TABLE versions (
    id           INTEGER PRIMARY KEY,
    game_id      INTEGER NOT NULL REFERENCES games(id) ON DELETE CASCADE,
    version      TEXT    NOT NULL,
    changelog    TEXT    NOT NULL,
    published_by INTEGER REFERENCES users(id) ON DELETE SET NULL,
    created_at   INTEGER NOT NULL,
    UNIQUE (game_id, version)
);
CREATE INDEX versions_game ON versions(game_id, id DESC);

-- One row per platform per version. The manifest is stored as JSON.
CREATE TABLE builds (
    id            INTEGER PRIMARY KEY,
    version_id    INTEGER NOT NULL REFERENCES versions(id) ON DELETE CASCADE,
    platform      TEXT    NOT NULL,
    manifest      TEXT    NOT NULL,
    manifest_hash TEXT    NOT NULL,
    total_size    INTEGER NOT NULL,
    file_count    INTEGER NOT NULL,
    launch        TEXT    NOT NULL,
    UNIQUE (version_id, platform)
);

CREATE TABLE library (
    user_id  INTEGER NOT NULL REFERENCES users(id) ON DELETE CASCADE,
    game_id  INTEGER NOT NULL REFERENCES games(id) ON DELETE CASCADE,
    added_at INTEGER NOT NULL,
    PRIMARY KEY (user_id, game_id)
);
