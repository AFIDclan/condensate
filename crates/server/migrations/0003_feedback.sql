-- Feedback recordings: a lobby's voice chat (saved per player, then transcribed) and the
-- screenshots players took, for the game's owner. Files live in <data>/feedback/<session id>/.
CREATE TABLE feedback_sessions (
    id         TEXT    PRIMARY KEY,
    game_id    INTEGER NOT NULL REFERENCES games(id) ON DELETE CASCADE,
    host_id    INTEGER NOT NULL REFERENCES users(id) ON DELETE CASCADE,
    started_at INTEGER NOT NULL,
    ended_at   INTEGER,
    -- recording | transcribing | ready | untranscribed | failed
    status     TEXT    NOT NULL,
    note       TEXT,
    version    TEXT
);
CREATE INDEX feedback_sessions_game ON feedback_sessions(game_id, started_at DESC);

CREATE TABLE feedback_utterances (
    id          INTEGER PRIMARY KEY,
    session_id  TEXT    NOT NULL REFERENCES feedback_sessions(id) ON DELETE CASCADE,
    user_id     INTEGER NOT NULL REFERENCES users(id) ON DELETE CASCADE,
    -- milliseconds from the start of the recording
    at_ms       INTEGER NOT NULL,
    duration_ms INTEGER NOT NULL,
    file        TEXT    NOT NULL,
    text        TEXT
);
CREATE INDEX feedback_utterances_session ON feedback_utterances(session_id, at_ms);

CREATE TABLE feedback_shots (
    id         INTEGER PRIMARY KEY,
    session_id TEXT    NOT NULL REFERENCES feedback_sessions(id) ON DELETE CASCADE,
    user_id    INTEGER NOT NULL REFERENCES users(id) ON DELETE CASCADE,
    at_ms      INTEGER NOT NULL,
    file       TEXT    NOT NULL,
    meta       TEXT    NOT NULL DEFAULT '{}'
);
CREATE INDEX feedback_shots_session ON feedback_shots(session_id, at_ms);
