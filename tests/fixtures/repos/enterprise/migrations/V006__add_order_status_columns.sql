-- Add status tracking columns to orders
-- Note: FK without covering index triggers PGM003

ALTER TABLE orders ADD COLUMN status_updated_by bigint;
ALTER TABLE orders ADD COLUMN status_updated_at timestamp(6);

ALTER TABLE orders ADD CONSTRAINT fk_orders_status_updated_by
    FOREIGN KEY (status_updated_by) REFERENCES users(id);
