-- Add support user fields to users table
-- Note: FK without covering index triggers PGM003

ALTER TABLE users ADD COLUMN support_user_id bigint;
ALTER TABLE users ADD COLUMN support_username varchar(100);

ALTER TABLE users ADD CONSTRAINT fk_users_support_user
    FOREIGN KEY (support_user_id) REFERENCES users(id);
