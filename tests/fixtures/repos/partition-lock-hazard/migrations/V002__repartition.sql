BEGIN;

-- Drop the quarterly 2027 partitions (children before parents)
-- 2027 Q1
DROP TABLE preliminary_postings_y2027q1;
DROP TABLE transaction_references_y2027q1;
DROP TABLE booking_information_y2027q1;
DROP TABLE preliminary_vouchers_y2027q1;
DROP TABLE transactions_y2027q1;

-- 2027 Q2
DROP TABLE preliminary_postings_y2027q2;
DROP TABLE transaction_references_y2027q2;
DROP TABLE booking_information_y2027q2;
DROP TABLE preliminary_vouchers_y2027q2;
DROP TABLE transactions_y2027q2;

-- 2027 Q3
DROP TABLE preliminary_postings_y2027q3;
DROP TABLE transaction_references_y2027q3;
DROP TABLE booking_information_y2027q3;
DROP TABLE preliminary_vouchers_y2027q3;
DROP TABLE transactions_y2027q3;

-- 2027 Q4
DROP TABLE preliminary_postings_y2027q4;
DROP TABLE transaction_references_y2027q4;
DROP TABLE booking_information_y2027q4;
DROP TABLE preliminary_vouchers_y2027q4;
DROP TABLE transactions_y2027q4;

-- Recreate the original yearly partitions for 2027 (parents before children),
-- with their full current index/constraint set.
CREATE TABLE transactions_y2027 PARTITION OF transactions FOR VALUES FROM ('2027-01-01') TO ('2028-01-01');
ALTER TABLE transactions_y2027 ADD CONSTRAINT transactions_pkey_y2027 PRIMARY KEY (id, partition_key);
CREATE INDEX ON transactions_y2027(tenant_id);
CREATE INDEX transactions_y2027_tenant_id_status_type_partition_key_idx ON transactions_y2027 USING btree (tenant_id,status,type,partition_key);
CREATE INDEX transactions_y2027_tenant_id_payment_date_idx ON transactions_y2027 USING btree (tenant_id,payment_date);
CREATE INDEX tenant_status_partial_y2027_idx ON transactions_y2027 (tenant_id) WHERE (status = 1 OR status = 0);
CREATE INDEX transactions_y2027_status_idx ON transactions_y2027 USING btree (status);

CREATE TABLE transaction_references_y2027 PARTITION OF transaction_references FOR VALUES FROM ('2027-01-01') TO ('2028-01-01');
ALTER TABLE transaction_references_y2027 ADD CONSTRAINT transaction_references_transaction_id_fkey_y2027 FOREIGN KEY (transaction_id, partition_key) REFERENCES transactions_y2027 (id, partition_key) ON DELETE CASCADE;
ALTER TABLE transaction_references_y2027 ADD PRIMARY KEY (id);
CREATE INDEX ON transaction_references_y2027(transaction_id);

CREATE TABLE booking_information_y2027 PARTITION OF booking_information FOR VALUES FROM ('2027-01-01') TO ('2028-01-01');
ALTER TABLE booking_information_y2027 ADD CONSTRAINT booking_information_transaction_id_fkey_y2027 FOREIGN KEY (transaction_id, partition_key) REFERENCES transactions_y2027 (id, partition_key) ON DELETE CASCADE;
ALTER TABLE booking_information_y2027 ADD PRIMARY KEY (id);
CREATE UNIQUE INDEX ON booking_information_y2027 (transaction_id, document_id, voucher_id, voucher_year, voucher_series, amount, amount_currency);
CREATE INDEX booking_information_y2027_tenant_id_idx ON booking_information_y2027 (tenant_id,voucher_year,voucher_series,voucher_id);
CREATE INDEX booking_information_y2027_created_idx ON booking_information_y2027 USING btree (created);

CREATE TABLE preliminary_vouchers_y2027 PARTITION OF preliminary_vouchers FOR VALUES FROM ('2027-01-01') TO ('2028-01-01');
ALTER TABLE preliminary_vouchers_y2027 ADD CONSTRAINT preliminary_vouchers_transactions_fkey_y2027 FOREIGN KEY (transaction_id, partition_key) REFERENCES transactions_y2027 (id, partition_key) ON DELETE CASCADE;

CREATE TABLE preliminary_postings_y2027 PARTITION OF preliminary_postings FOR VALUES FROM ('2027-01-01') TO ('2028-01-01');
ALTER TABLE preliminary_postings_y2027 ADD CONSTRAINT preliminary_postings_preliminary_vouchers_fkey_y2027 FOREIGN KEY (transaction_id, partition_key) REFERENCES preliminary_vouchers_y2027 (transaction_id, partition_key) ON DELETE CASCADE;

COMMIT;
