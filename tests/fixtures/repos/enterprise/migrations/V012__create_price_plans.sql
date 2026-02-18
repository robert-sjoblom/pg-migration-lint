-- Price plan tables
-- Note: now() defaults on new tables trigger PGM006

CREATE TABLE price_plans (
    id serial PRIMARY KEY,
    name text NOT NULL UNIQUE,
    created_at timestamptz NOT NULL DEFAULT now()
);

CREATE TABLE price_plan_items (
    id serial PRIMARY KEY,
    price_plan_id integer NOT NULL REFERENCES price_plans(id),
    product_id integer NOT NULL REFERENCES products(id),
    modification_type text NOT NULL,
    modification_value integer NOT NULL,
    created_at timestamptz NOT NULL DEFAULT now()
);

CREATE TABLE subscription_price_plan (
    id serial PRIMARY KEY,
    account_id bigint NOT NULL,
    price_plan_id integer NOT NULL REFERENCES price_plans(id),
    created_at timestamptz NOT NULL DEFAULT now()
);

CREATE INDEX idx_price_plan_items_plan ON price_plan_items (price_plan_id);
CREATE INDEX idx_subscription_price_plan_account ON subscription_price_plan (account_id);
