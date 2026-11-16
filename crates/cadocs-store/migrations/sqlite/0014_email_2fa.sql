-- A second factor by mail (ADR 0026): a login then needs a code sent to the address. Not asked
-- while the instance has no relay.
ALTER TABLE users ADD COLUMN email_2fa SMALLINT NOT NULL DEFAULT 0 CHECK (email_2fa IN (0, 1));
