CREATE TABLE customers (id bigint PRIMARY KEY);

CREATE TABLE orders (
    id bigint PRIMARY KEY,
    customer_id bigint NOT NULL,
    status text NOT NULL
);

-- Named FK without covering index (NOT VALID for PGM014 safe pattern)
ALTER TABLE orders ADD CONSTRAINT fk_customer
    FOREIGN KEY (customer_id) REFERENCES customers(id) NOT VALID;

-- Named CHECK (NOT VALID)
ALTER TABLE orders ADD CONSTRAINT chk_status
    CHECK (status IN ('pending', 'shipped')) NOT VALID;

-- Table with UNIQUE NOT NULL but no PK (PGM503 candidate)
CREATE TABLE settings (
    key text NOT NULL,
    value text,
    CONSTRAINT uq_key UNIQUE (key)
);
