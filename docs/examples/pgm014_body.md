Detects `ALTER TABLE ... ADD CONSTRAINT ... FOREIGN KEY` on a pre-existing table without the `NOT VALID` modifier. Without `NOT VALID`, PostgreSQL immediately validates all existing rows under a SHARE ROW EXCLUSIVE lock on both the referencing and the referenced table.

**Example** (bad):
```sql
ALTER TABLE orders
  ADD CONSTRAINT fk_customer
  FOREIGN KEY (customer_id) REFERENCES customers (id);
```

**Fix** (safe pattern):
```sql
ALTER TABLE orders
  ADD CONSTRAINT fk_customer
  FOREIGN KEY (customer_id) REFERENCES customers (id)
  NOT VALID;
ALTER TABLE orders VALIDATE CONSTRAINT fk_customer;
```

See also [PGM015](#pgm015).
