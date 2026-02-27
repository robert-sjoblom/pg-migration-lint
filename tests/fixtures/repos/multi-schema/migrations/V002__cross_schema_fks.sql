-- Cross-schema FK: billing → auth
ALTER TABLE billing.invoices
    ADD CONSTRAINT fk_invoices_user
    FOREIGN KEY (user_id) REFERENCES auth.users(id);

-- Same-schema FK: inventory internal
ALTER TABLE inventory.stock
    ADD CONSTRAINT fk_stock_product
    FOREIGN KEY (product_id) REFERENCES inventory.products(id);

-- Unqualified table FK → qualified schema
ALTER TABLE orders
    ADD CONSTRAINT fk_orders_product
    FOREIGN KEY (product_id) REFERENCES inventory.products(id);

-- Covering index for stock FK (proves per-table index tracking)
CREATE INDEX idx_stock_product ON inventory.stock (product_id);
