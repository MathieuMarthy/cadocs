-- Mail notifications (step 42): a user's choice, and which notifications a message already told.
ALTER TABLE users ADD COLUMN email_notifications SMALLINT NOT NULL DEFAULT 0
    CHECK (email_notifications IN (0, 1));
ALTER TABLE notifications ADD COLUMN mailed_at BIGINT;
