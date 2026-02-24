Detects foreign key constraints where the referencing table has no index whose leading columns match the FK columns in order. Without such an index, deletes and updates on the referenced table cause sequential scans on the referencing table.

**Example** (bad):
```sql
ALTER TABLE order_items
  ADD CONSTRAINT fk_order
  FOREIGN KEY (order_id) REFERENCES orders(id);
-- No index on order_items(order_id)
```

**Fix**:
```sql
CREATE INDEX idx_order_items_order_id ON order_items (order_id);
ALTER TABLE order_items
  ADD CONSTRAINT fk_order
  FOREIGN KEY (order_id) REFERENCES orders(id);
```

Uses prefix matching: FK columns `(a, b)` are covered by index `(a, b)` or `(a, b, c)` but **not** by `(b, a)` or `(a)`. Column order matters. The check uses the catalog state after the entire file is processed, so creating the index later in the same file avoids a false positive.
