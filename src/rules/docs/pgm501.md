Detects foreign key constraints where the referencing table has no usable index containing any of the FK columns. Without such an index, any query joining or filtering on those columns forces a sequential scan on the referencing table — and so does a referential integrity check when a referenced row is deleted or its key columns are updated.

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

**Column matching**: FK columns `(a, b)` are covered by any usable index that contains at least one of `a` or `b`, in any position — e.g. `(a, b)`, `(b, a)`, `(a)`, or `(c, b)` all count. An index covering only some of the FK columns still avoids a sequential scan (via a Filter or Recheck), so it counts as coverage even though a fully covering index performs better. Column order does not matter here. The check uses the catalog state after the entire file is processed, so creating the index later in the same file avoids a false positive.

**Known limitation**: an FK whose only indexed column is low-cardinality (e.g. 10 distinct values over 1M rows) produces no warning, even though the check still degrades to a Bitmap Heap Scan touching a large fraction of the table. Index shape alone can't tell a selective index from an unselective one — the catalog has no column statistics, and requiring the index to also be UNIQUE would not help, since the false-negative case is not unique in the referencing table either.
