-- Promotion tables with foreign keys
-- Only index on promotion_id, not product_id: triggers PGM501 for product_id FK

CREATE TABLE promotions (
    id uuid PRIMARY KEY,
    name varchar(200) NOT NULL,
    promotion_type varchar(50) NOT NULL,
    status varchar(50) NOT NULL DEFAULT 'DRAFT',
    valid_from date,
    valid_until date,
    created timestamp(6) NOT NULL
);

CREATE TABLE promotion_products (
    id integer PRIMARY KEY,
    promotion_id uuid NOT NULL,
    product_id integer NOT NULL,
    max_amount integer NOT NULL DEFAULT 1,
    discount_percent numeric(5,2),
    CONSTRAINT fk_promo_products_promotion FOREIGN KEY (promotion_id) REFERENCES promotions(id),
    CONSTRAINT fk_promo_products_product FOREIGN KEY (product_id) REFERENCES products(id)
);

-- Only index on promotion_id, not product_id
CREATE INDEX idx_promotion_products_promo ON promotion_products (promotion_id);
