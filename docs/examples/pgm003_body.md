Detects `CREATE INDEX CONCURRENTLY`, `DROP INDEX CONCURRENTLY`, `ALTER TABLE ... DETACH PARTITION ... CONCURRENTLY`, or `REINDEX ... CONCURRENTLY` inside a migration unit that runs in a transaction. PostgreSQL does not allow any of these CONCURRENTLY operations inside a transaction block — the command will fail at runtime.

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

See also [PGM001](#pgm001), [PGM002](#pgm002), and [PGM022](#pgm022).
