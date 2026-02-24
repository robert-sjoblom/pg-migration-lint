Detects `CREATE INDEX CONCURRENTLY` or `DROP INDEX CONCURRENTLY` inside a migration unit that runs in a transaction. PostgreSQL does not allow concurrent index operations inside a transaction block — the command will fail at runtime.

**Example** (bad — Liquibase changeset with default `runInTransaction`):
```xml
<changeSet id="1" author="dev">
  <sql>CREATE INDEX CONCURRENTLY idx_foo ON bar (col);</sql>
</changeSet>
```

**Fix**:
```xml
<changeSet id="1" author="dev" runInTransaction="false">
  <sql>CREATE INDEX CONCURRENTLY idx_foo ON bar (col);</sql>
</changeSet>
```

See also [PGM001](#pgm001) and [PGM002](#pgm002).
