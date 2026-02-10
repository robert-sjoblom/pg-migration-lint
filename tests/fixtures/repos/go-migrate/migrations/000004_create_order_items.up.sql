CREATE TABLE order_items (
    id UUID PRIMARY KEY,
    order_id UUID NOT NULL REFERENCES orders (id) ON DELETE CASCADE,
    product_name TEXT NOT NULL,
    quantity INT NOT NULL DEFAULT 1,
    unit_price FLOAT NOT NULL,
    created TIMESTAMPTZ DEFAULT CURRENT_TIMESTAMP
);

CREATE INDEX idx_order_items_order ON order_items (order_id);
