-- A second factor from an authenticator app (RFC 6238). The secret stays readable: the server
-- computes the codes from it. A row with no `enabled_at` is a setup waiting for its first code.
CREATE TABLE user_totp (
    user_id    TEXT PRIMARY KEY REFERENCES users(id) ON DELETE CASCADE,
    secret     BLOB NOT NULL,
    enabled_at BIGINT,
    last_step  BIGINT NOT NULL DEFAULT 0,    -- the time step of the last code used: none works twice
    created_at BIGINT NOT NULL
);

-- Single use, in place of a code from the app. SHA-256 of the code, which holds 80 random bits.
CREATE TABLE user_recovery_codes (
    user_id   TEXT NOT NULL REFERENCES users(id) ON DELETE CASCADE,
    code_hash TEXT NOT NULL,
    PRIMARY KEY (user_id, code_hash)
);
