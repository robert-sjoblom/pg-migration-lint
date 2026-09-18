Detects columns declared as `char(n)` or `character(n)`. In PostgreSQL, `char(n)` pads with trailing spaces, wastes storage, and is no faster than `text` or `varchar`.

**Example** (bad):
```sql
CREATE TABLE countries (code char(2) NOT NULL);
```

**Fix**:
```sql
CREATE TABLE countries (code text NOT NULL);
-- or: code varchar(2) NOT NULL
```
