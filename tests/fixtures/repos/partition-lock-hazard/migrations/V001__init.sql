-- Initial partitioned schema (quarterly partitions for 2027)
CREATE TABLE transactions (
    id uuid NOT NULL,
    partition_key date NOT NULL,
    tenant_id bigint NOT NULL,
    status integer NOT NULL,
    type integer NOT NULL,
    payment_date date,
    created timestamptz NOT NULL DEFAULT now()
) PARTITION BY RANGE (partition_key);

CREATE TABLE transaction_references (
    id uuid NOT NULL,
    transaction_id uuid NOT NULL,
    partition_key date NOT NULL,
    reference text
) PARTITION BY RANGE (partition_key);

CREATE TABLE booking_information (
    id uuid NOT NULL,
    transaction_id uuid NOT NULL,
    partition_key date NOT NULL,
    tenant_id bigint NOT NULL,
    document_id uuid,
    voucher_id bigint,
    voucher_year integer,
    voucher_series text,
    amount numeric,
    amount_currency text,
    created timestamptz NOT NULL DEFAULT now()
) PARTITION BY RANGE (partition_key);

CREATE TABLE preliminary_vouchers (
    id uuid NOT NULL,
    transaction_id uuid NOT NULL,
    partition_key date NOT NULL
) PARTITION BY RANGE (partition_key);

CREATE TABLE preliminary_postings (
    id uuid NOT NULL,
    transaction_id uuid NOT NULL,
    partition_key date NOT NULL,
    account integer
) PARTITION BY RANGE (partition_key);

-- 2027 Q1
CREATE TABLE transactions_y2027q1 PARTITION OF transactions FOR VALUES FROM ('2027-01-01') TO ('2027-04-01');
ALTER TABLE transactions_y2027q1 ADD CONSTRAINT transactions_pkey_y2027q1 PRIMARY KEY (id, partition_key);
CREATE INDEX ON transactions_y2027q1(tenant_id);
CREATE TABLE transaction_references_y2027q1 PARTITION OF transaction_references FOR VALUES FROM ('2027-01-01') TO ('2027-04-01');
ALTER TABLE transaction_references_y2027q1 ADD CONSTRAINT transaction_references_transaction_id_fkey_y2027q1 FOREIGN KEY (transaction_id, partition_key) REFERENCES transactions_y2027q1 (id, partition_key) ON DELETE CASCADE;
CREATE INDEX ON transaction_references_y2027q1(transaction_id);
CREATE TABLE booking_information_y2027q1 PARTITION OF booking_information FOR VALUES FROM ('2027-01-01') TO ('2027-04-01');
ALTER TABLE booking_information_y2027q1 ADD CONSTRAINT booking_information_transaction_id_fkey_y2027q1 FOREIGN KEY (transaction_id, partition_key) REFERENCES transactions_y2027q1 (id, partition_key) ON DELETE CASCADE;
CREATE TABLE preliminary_vouchers_y2027q1 PARTITION OF preliminary_vouchers FOR VALUES FROM ('2027-01-01') TO ('2027-04-01');
ALTER TABLE preliminary_vouchers_y2027q1 ADD CONSTRAINT preliminary_vouchers_transactions_fkey_y2027q1 FOREIGN KEY (transaction_id, partition_key) REFERENCES transactions_y2027q1 (id, partition_key) ON DELETE CASCADE;
CREATE TABLE preliminary_postings_y2027q1 PARTITION OF preliminary_postings FOR VALUES FROM ('2027-01-01') TO ('2027-04-01');
ALTER TABLE preliminary_postings_y2027q1 ADD CONSTRAINT preliminary_postings_preliminary_vouchers_fkey_y2027q1 FOREIGN KEY (transaction_id, partition_key) REFERENCES preliminary_vouchers_y2027q1 (transaction_id, partition_key) ON DELETE CASCADE;

-- 2027 Q2
CREATE TABLE transactions_y2027q2 PARTITION OF transactions FOR VALUES FROM ('2027-04-01') TO ('2027-07-01');
ALTER TABLE transactions_y2027q2 ADD CONSTRAINT transactions_pkey_y2027q2 PRIMARY KEY (id, partition_key);
CREATE INDEX ON transactions_y2027q2(tenant_id);
CREATE TABLE transaction_references_y2027q2 PARTITION OF transaction_references FOR VALUES FROM ('2027-04-01') TO ('2027-07-01');
ALTER TABLE transaction_references_y2027q2 ADD CONSTRAINT transaction_references_transaction_id_fkey_y2027q2 FOREIGN KEY (transaction_id, partition_key) REFERENCES transactions_y2027q2 (id, partition_key) ON DELETE CASCADE;
CREATE INDEX ON transaction_references_y2027q2(transaction_id);
CREATE TABLE booking_information_y2027q2 PARTITION OF booking_information FOR VALUES FROM ('2027-04-01') TO ('2027-07-01');
ALTER TABLE booking_information_y2027q2 ADD CONSTRAINT booking_information_transaction_id_fkey_y2027q2 FOREIGN KEY (transaction_id, partition_key) REFERENCES transactions_y2027q2 (id, partition_key) ON DELETE CASCADE;
CREATE TABLE preliminary_vouchers_y2027q2 PARTITION OF preliminary_vouchers FOR VALUES FROM ('2027-04-01') TO ('2027-07-01');
ALTER TABLE preliminary_vouchers_y2027q2 ADD CONSTRAINT preliminary_vouchers_transactions_fkey_y2027q2 FOREIGN KEY (transaction_id, partition_key) REFERENCES transactions_y2027q2 (id, partition_key) ON DELETE CASCADE;
CREATE TABLE preliminary_postings_y2027q2 PARTITION OF preliminary_postings FOR VALUES FROM ('2027-04-01') TO ('2027-07-01');
ALTER TABLE preliminary_postings_y2027q2 ADD CONSTRAINT preliminary_postings_preliminary_vouchers_fkey_y2027q2 FOREIGN KEY (transaction_id, partition_key) REFERENCES preliminary_vouchers_y2027q2 (transaction_id, partition_key) ON DELETE CASCADE;

-- 2027 Q3
CREATE TABLE transactions_y2027q3 PARTITION OF transactions FOR VALUES FROM ('2027-07-01') TO ('2027-10-01');
ALTER TABLE transactions_y2027q3 ADD CONSTRAINT transactions_pkey_y2027q3 PRIMARY KEY (id, partition_key);
CREATE INDEX ON transactions_y2027q3(tenant_id);
CREATE TABLE transaction_references_y2027q3 PARTITION OF transaction_references FOR VALUES FROM ('2027-07-01') TO ('2027-10-01');
ALTER TABLE transaction_references_y2027q3 ADD CONSTRAINT transaction_references_transaction_id_fkey_y2027q3 FOREIGN KEY (transaction_id, partition_key) REFERENCES transactions_y2027q3 (id, partition_key) ON DELETE CASCADE;
CREATE INDEX ON transaction_references_y2027q3(transaction_id);
CREATE TABLE booking_information_y2027q3 PARTITION OF booking_information FOR VALUES FROM ('2027-07-01') TO ('2027-10-01');
ALTER TABLE booking_information_y2027q3 ADD CONSTRAINT booking_information_transaction_id_fkey_y2027q3 FOREIGN KEY (transaction_id, partition_key) REFERENCES transactions_y2027q3 (id, partition_key) ON DELETE CASCADE;
CREATE TABLE preliminary_vouchers_y2027q3 PARTITION OF preliminary_vouchers FOR VALUES FROM ('2027-07-01') TO ('2027-10-01');
ALTER TABLE preliminary_vouchers_y2027q3 ADD CONSTRAINT preliminary_vouchers_transactions_fkey_y2027q3 FOREIGN KEY (transaction_id, partition_key) REFERENCES transactions_y2027q3 (id, partition_key) ON DELETE CASCADE;
CREATE TABLE preliminary_postings_y2027q3 PARTITION OF preliminary_postings FOR VALUES FROM ('2027-07-01') TO ('2027-10-01');
ALTER TABLE preliminary_postings_y2027q3 ADD CONSTRAINT preliminary_postings_preliminary_vouchers_fkey_y2027q3 FOREIGN KEY (transaction_id, partition_key) REFERENCES preliminary_vouchers_y2027q3 (transaction_id, partition_key) ON DELETE CASCADE;

-- 2027 Q4
CREATE TABLE transactions_y2027q4 PARTITION OF transactions FOR VALUES FROM ('2027-10-01') TO ('2028-01-01');
ALTER TABLE transactions_y2027q4 ADD CONSTRAINT transactions_pkey_y2027q4 PRIMARY KEY (id, partition_key);
CREATE INDEX ON transactions_y2027q4(tenant_id);
CREATE TABLE transaction_references_y2027q4 PARTITION OF transaction_references FOR VALUES FROM ('2027-10-01') TO ('2028-01-01');
ALTER TABLE transaction_references_y2027q4 ADD CONSTRAINT transaction_references_transaction_id_fkey_y2027q4 FOREIGN KEY (transaction_id, partition_key) REFERENCES transactions_y2027q4 (id, partition_key) ON DELETE CASCADE;
CREATE INDEX ON transaction_references_y2027q4(transaction_id);
CREATE TABLE booking_information_y2027q4 PARTITION OF booking_information FOR VALUES FROM ('2027-10-01') TO ('2028-01-01');
ALTER TABLE booking_information_y2027q4 ADD CONSTRAINT booking_information_transaction_id_fkey_y2027q4 FOREIGN KEY (transaction_id, partition_key) REFERENCES transactions_y2027q4 (id, partition_key) ON DELETE CASCADE;
CREATE TABLE preliminary_vouchers_y2027q4 PARTITION OF preliminary_vouchers FOR VALUES FROM ('2027-10-01') TO ('2028-01-01');
ALTER TABLE preliminary_vouchers_y2027q4 ADD CONSTRAINT preliminary_vouchers_transactions_fkey_y2027q4 FOREIGN KEY (transaction_id, partition_key) REFERENCES transactions_y2027q4 (id, partition_key) ON DELETE CASCADE;
CREATE TABLE preliminary_postings_y2027q4 PARTITION OF preliminary_postings FOR VALUES FROM ('2027-10-01') TO ('2028-01-01');
ALTER TABLE preliminary_postings_y2027q4 ADD CONSTRAINT preliminary_postings_preliminary_vouchers_fkey_y2027q4 FOREIGN KEY (transaction_id, partition_key) REFERENCES preliminary_vouchers_y2027q4 (transaction_id, partition_key) ON DELETE CASCADE;
