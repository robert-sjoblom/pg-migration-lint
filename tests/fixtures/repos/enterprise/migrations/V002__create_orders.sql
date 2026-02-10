-- Main order tables with foreign keys and inline constraints

CREATE TABLE orders (
    id bigint PRIMARY KEY,
    account_id bigint NOT NULL,
    user_id bigint REFERENCES users(id),
    order_type varchar(50) NOT NULL,
    status varchar(50) NOT NULL DEFAULT 'PENDING',
    products jsonb,
    notes text,
    created timestamp(6) NOT NULL,
    updated timestamp(6)
);

CREATE TABLE internal_orders (
    id bigint PRIMARY KEY REFERENCES orders(id),
    team varchar(100),
    order_value numeric(12,2),
    approved_by bigint REFERENCES users(id)
);

CREATE TABLE partner_client_orders (
    order_id bigint PRIMARY KEY REFERENCES orders(id),
    partner_account_id bigint NOT NULL,
    client_account_id bigint NOT NULL,
    partner_reference varchar(100)
);

CREATE TABLE order_actions (
    id uuid PRIMARY KEY,
    order_id bigint NOT NULL REFERENCES orders(id),
    action_type varchar(50) NOT NULL,
    status varchar(50) NOT NULL,
    result varchar(50),
    performed_by bigint REFERENCES users(id),
    created timestamp(6) NOT NULL
);

CREATE INDEX idx_order_actions_order_id ON order_actions (order_id);
CREATE INDEX idx_orders_account_id ON orders (account_id);
