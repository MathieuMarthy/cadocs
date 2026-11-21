-- Accounts an OpenID Connect provider vouches for: its issuer's subject, never an address.
CREATE TABLE user_identities (
    provider   TEXT NOT NULL,
    subject    TEXT NOT NULL,
    user_id    TEXT NOT NULL REFERENCES users(id) ON DELETE CASCADE,
    created_at BIGINT NOT NULL,
    PRIMARY KEY (provider, subject)
);
CREATE INDEX user_identities_user ON user_identities(user_id);
