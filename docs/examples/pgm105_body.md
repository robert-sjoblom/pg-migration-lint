Detects columns declared as `serial`, `bigserial`, or `smallserial`. Since PostgreSQL 10, identity columns (`GENERATED ALWAYS AS IDENTITY`) provide the same auto-incrementing behavior with tighter ownership, better permission handling, and SQL standard compliance.

**Example** (flagged):
```sql
CREATE TABLE orders (id serial PRIMARY KEY);
```

**Fix**:
```sql
CREATE TABLE orders (
  id bigint GENERATED ALWAYS AS IDENTITY PRIMARY KEY
);
```
