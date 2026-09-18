Detects table and column names that require perpetual double-quoting — either because they contain uppercase characters or because they match a PostgreSQL reserved word.

**Example** (flagged):
```sql
CREATE TABLE "User" ("Id" bigint, "order" text);
-- Every query must now use: SELECT "Id", "order" FROM "User";
```

**Why it matters**:
- Every query must use the exact case and double-quotes — forgetting them silently references a different (lowercased) identifier.
- IDE autocompletion and ORMs may generate incorrect SQL.
- `pg_dump` output becomes harder to read and modify.

**Does NOT fire when**:
- The identifier is all-lowercase and not a PostgreSQL reserved word.
- The identifier is a schema name or index name (only table and column names are checked).

**Fix**: Use a lowercase, non-reserved name:
```sql
CREATE TABLE users (id bigint, order_status text);
```
