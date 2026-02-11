-- Cross-schema FK: unqualified orders references schema-qualified customers
ALTER TABLE orders ADD CONSTRAINT fk_orders_customer
    FOREIGN KEY (customer_id) REFERENCES myschema.customers(id);

-- Add covering index so PGM003 doesn't fire
CREATE INDEX idx_orders_customer_id ON orders (customer_id);
