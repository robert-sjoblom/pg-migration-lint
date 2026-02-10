-- PGM003: Foreign key without covering index
ALTER TABLE orders ADD COLUMN assigned_user_id UUID;
ALTER TABLE orders ADD CONSTRAINT fk_orders_assigned_user
    FOREIGN KEY (assigned_user_id) REFERENCES users (id);
