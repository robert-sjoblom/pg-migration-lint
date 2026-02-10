CREATE TABLE orders (
    id bigint PRIMARY KEY,
    user_id bigint NOT NULL REFERENCES users(id),
    total numeric(10,2) NOT NULL DEFAULT 0
);
CREATE INDEX idx_orders_user_id ON orders (user_id);
