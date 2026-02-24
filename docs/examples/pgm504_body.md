Detects `ALTER TABLE ... RENAME TO` on a pre-existing table. Renaming breaks all queries, views, and functions referencing the old name. The rename itself is instant DDL (metadata-only), but downstream breakage can be severe.

**Example** (bad):
```sql
ALTER TABLE orders RENAME TO orders_archive;
-- All queries referencing 'orders' will fail.
```

**Fix** (backward-compatible):
```sql
ALTER TABLE orders RENAME TO orders_v2;
CREATE VIEW orders AS SELECT * FROM orders_v2;
```

Does not fire when a replacement table with the old name is created in the same migration unit (safe swap pattern).
