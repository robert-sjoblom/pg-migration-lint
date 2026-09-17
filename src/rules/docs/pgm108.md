Detects columns declared as `varchar(n)` or `character varying(n)`. In PostgreSQL, `varchar(n)` has zero performance benefit over `text` — they share identical `varlena` storage. The length constraint adds an artificial limit that may require future schema changes.

**Example** (bad):
```sql
CREATE TABLE users (name varchar(100) NOT NULL);
```

**Fix**:
```sql
CREATE TABLE users (name text NOT NULL);
-- If validation is needed, use a CHECK constraint:
-- ALTER TABLE users ADD CONSTRAINT chk_name_len CHECK (length(name) <= 100) NOT VALID;
-- ALTER TABLE users VALIDATE CONSTRAINT chk_name_len;
```
