-- Codes sent by mail (ADR 0026). An address is verified once a code sent to it was used: accounts
-- made before this, or from an invitation, keep NULL.
ALTER TABLE users ADD COLUMN email_verified_at BIGINT;

-- One code per purpose and address: asking again replaces it. `login` and `email` are for the
-- second factor and the proof of a new address, which come later.
CREATE TABLE email_codes (
    purpose    TEXT NOT NULL CHECK (purpose IN ('register', 'login', 'email')),
    email      TEXT NOT NULL,                -- stored lowercased, where the code went
    user_id    TEXT REFERENCES users(id) ON DELETE CASCADE, -- NULL for a registration
    code_hash  TEXT NOT NULL,                -- keyed SHA-256, with a key new at each start
    tries      INTEGER NOT NULL DEFAULT 0,   -- wrong ones; the 5th deletes the row
    expires_at BIGINT NOT NULL,
    created_at BIGINT NOT NULL,
    PRIMARY KEY (purpose, email)
);
CREATE INDEX email_codes_expiry ON email_codes(expires_at);
CREATE INDEX email_codes_user ON email_codes(user_id);
