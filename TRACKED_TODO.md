# TODO

# Partition lock-hazard work (2026-09-01)

All of the below was implemented in one working copy, then reset deliberately to get one concern per
commit. The changes are gone; these notes are the source of truth for redoing them. Suggested order is
top to bottom — the docs entry and the PGM501 entries are the only ones with ordering constraints
between them.

**Any change to a rule's `EXPLAIN` requires a docgen pass afterwards:** `cargo test --features docgen`,
accept `src/snapshots/pg_migration_lint__docgen__tests__rules_md.snap`, then `make docs-sync` to
regenerate `docs/rules.md`. Do not hand-edit `docs/rules.md`.

## Measured PostgreSQL facts these entries rely on

Established against real PostgreSQL 14–18; the partition ones are encoded as tests in
`verify/lock-tests/tests/partition_*.rs`. Do not re-derive.

Lock taken **on the parent** partitioned table:

| statement | lock on parent |
|---|---|
| `DROP TABLE <child>` | `AccessExclusive` |
| `CREATE TABLE <child> PARTITION OF <parent>` | `AccessExclusive` |
| `ALTER TABLE <parent> DETACH PARTITION <child>` | `AccessExclusive` |
| `ALTER TABLE <parent> ATTACH PARTITION <existing>` | `ShareUpdateExclusive` (and `AccessExclusive` on the table being attached) |

- A reader or writer holding the parent makes `DROP TABLE <child>` fail with SQLSTATE `55P03`.
- One `BEGIN…COMMIT` holds `AccessExclusive` on **every** parent it has touched until COMMIT, so one
  un-gettable lock discards all prior work in the migration.
- A merely *queued* `AccessExclusive` blocks new readers the current holder would have allowed.
- Traffic aimed straight at a sibling partition does **not** block the drop; routing through the parent
  is what makes it fail.
- `DROP TABLE <parent>` with **no** CASCADE succeeds and drops every partition with it (measured PG 17).
- `DETACH … CONCURRENTLY`: cannot run in a transaction block; still waits for traffic (fails `55P03`);
  phase one commits, so an interrupt leaves `pg_inherits.inhdetachpending` set; queries through the
  parent already skip the partition's rows while the rows remain in it; it never self-resolves; a retry
  is refused with SQLSTATE `55000` (hint names FINALIZE); exits are `DETACH … FINALIZE` or
  `DROP TABLE <child>` — but the pending child is still in `pg_inherits`, so its `DROP TABLE` still takes
  `AccessExclusive` on the parent and still loses to the traffic that caused the pending state.

Foreign-key index facts (FK enforcement query is `SELECT 1 FROM child WHERE a=$1 AND b=$2 FOR KEY SHARE`),
measured at 1M rows with `a` unique and `b` only 10 distinct values:

| index on referencing table | plan |
|---|---|
| none | `Seq Scan` |
| `(a,b)` | `Index Scan`, Cond on both |
| `(b,a)` — reversed | `Index Scan`, Cond on both — **column order is irrelevant when all FK columns are equality-constrained** |
| `(a)` — selective subset | `Index Scan`, Cond `a`, Filter `b` |
| `(b)` — unselective subset | `Bitmap Heap Scan`, Recheck `b`, Filter `a` |
| `(c,a,b)` — unrelated leading column | `Index Scan` with conds on a,b but scanning the **whole index** |

Also measured, via `pg_stat_user_tables.seq_scan` deltas: `INSERT` into the referencing table causes **0**
scans of it (the check looks up the *referenced* table's PK index); a **non-key** `UPDATE` of a referenced
row causes **0**; a `DELETE`, or an `UPDATE` of the referenced **key**, causes one scan without an index
and none with one.

### Foreign keys on *partitioned* referencing tables (measured 2026-09-01, PG 14–18, identical on all five)

The RI check deliberately omits `ONLY` when the referencing table is partitioned, so it queries the whole
hierarchy:

```
SELECT 1 FROM "public"."kid" x WHERE $1 OPERATOR(pg_catalog.=) "tenant_id"
                               AND $2 OPERATOR(pg_catalog.=) "period" FOR KEY SHARE OF x
```

- **Partition pruning is driven by the FK column list, never by an index.** The only quals in that query
  are the FK columns; if the partition key is not among them there is nothing to prune on. No index shape
  changes this either way.
- Partition key **not** in the FK → `Append` over **every** partition, one index scan each, and
  `RowShareLock` on every partition *and* every partition index. Linear in partition count: one `DELETE`
  of one referenced row against a 200-partition table is 200 index scans.
- Partition key **in** the FK → pruned to the one partition. Both mechanisms work: plan-time pruning with
  custom plans, init pruning (`Subplans Removed: 3`) once the cached plan goes generic. RANGE and LIST alike.
- `ON DELETE CASCADE` has the same fan-out — `DELETE FROM "public"."kid" WHERE $1 = "ref_id"`.
- The referenced side carries **one** trigger pair for the whole FK (2 `pg_trigger` rows), not one per leaf,
  even though `pg_constraint` holds one row per partition plus the parent. So it is a single query fanning
  out over N partitions, not N queries.
- The unpruned case degrades further once the plan goes generic: per-partition estimates lose the constant
  (`rows=200`) and the per-partition Index Scans become Bitmap Heap Scans.

Which columns the covering index needs **inside** the one surviving partition. RANGE-partitioned on
`period`, FK `(tenant_id, period)`, 4 partitions × 10k rows, 50 distinct `period` values per partition,
probing an absent pair whose individual column values are both common:

| index on the partition | plan | rows filtered | buffers |
|---|---|---|---|
| `(tenant_id)` — partition key absent | Bitmap Heap Scan, Recheck `tenant_id`, Filter `period` | 2500 | 57 |
| `(period)` — partition key only | Bitmap Heap Scan, Recheck `period`, Filter `tenant_id` | 200 | 56 |
| `(tenant_id, period)` — both | Bitmap Index Scan, Cond on both, never reaches the heap | 0 | **2** |

- **The partition key belongs in the covering index** — not for pruning, but for selectivity inside the
  partition. Under RANGE (and HASH) a partition holds many distinct partition-key values, so the column is
  genuinely selective there. Only **single-value LIST** makes it constant within a partition and therefore
  dead weight; under that shape all three indexes above cost 2 buffers and an index on the partition key
  alone is worthless (Seq Scan, 55 buffers). Do not generalise the LIST result — it was measured first and
  is the misleading one.
- **Partial coverage buys almost nothing.** `(period)` narrowed 12x harder than `(tenant_id)` — 200
  candidate rows against 2500 — and saved exactly **one** buffer, because the bitmap heap scan touched the
  same 54 heap blocks either way. The 28x win only arrives when *every* FK column is in the index.
- Column order among the FK columns stays irrelevant on partitioned tables too (every qual is equality),
  consistent with the `(b,a)` row in the table above.
- A **UNIQUE** covering index on a partitioned table *must* include the partition key; PostgreSQL refuses
  otherwise with `ERROR: unique constraint on partitioned table must include all partitioning columns`.
  Not a design choice.

**Ordinary joins on the FK columns behave the same way, but prune by a different mechanism.** A join has no
constant to prune on at plan time, so pruning becomes execution-time and depends entirely on the join
strategy. Same tables, joining a 2-row driver table on `(tenant_id, period)`:

| join shape | index | pruning | buffers |
|---|---|---|---|
| Nested Loop | `(tenant_id, period)` | yes — unmatched partitions show `(never executed)` | **6** |
| Nested Loop | `(tenant_id)` only | yes, same pruning — then Bitmap Heap Scan, `Rows Removed by Filter: 2400` | 116 |
| Hash Join (`enable_nestloop = off`) | `(tenant_id, period)` | **none** — `Append` seq-scans all 4 partitions, 40000 rows | 221 |

- Nested-loop pruning is **per outer row**, so it scales with the number of distinct partition-key values in
  the outer relation, not with partition count. This is the same pruning the RI check gets, since a
  parameterized inner scan and an RI query are both partition-key-equality-against-a-param.
- A hash or merge join gets **no pruning at all** — it has to read every partition to build or probe. Not a
  defect; it is what the join needs. But it means "the FK columns are indexed" buys nothing there, and the
  partition count multiplies the scan.
- The covering-index conclusion is unchanged and the ratio is comparable (19x here, 28x for the RI check):
  pruning is independent of the index, and the index still has to name *all* the joined columns to keep the
  scan off the heap. Indexing only the non-partition-key column left 2400 rows per outer row to filter.
- `enable_partitionwise_join` is a separate mechanism, off by default, and needs both sides partitioned
  compatibly — not measured here, and not what the FK case exercises.

Reproduction, inlined because the probe scripts live in a session scratchpad that will not survive. The
checkerboard population is the point: it makes each column value common while half the *pairs* are absent,
which is what separates the three index shapes.

```sql
CREATE TABLE ref_y (tenant_id int, period int, PRIMARY KEY (tenant_id, period));
INSERT INTO ref_y SELECT t, p FROM generate_series(1,4) t, generate_series(1,200) p;
CREATE TABLE kid (tenant_id int, period int NOT NULL, payload text,
  FOREIGN KEY (tenant_id, period) REFERENCES ref_y (tenant_id, period)) PARTITION BY RANGE (period);
CREATE TABLE kid_p2 PARTITION OF kid FOR VALUES FROM (51) TO (101);   -- plus p1/p3/p4
CREATE INDEX ON kid (tenant_id);          -- vary this line: the whole experiment is here
INSERT INTO kid SELECT t, p, 'x' FROM generate_series(1,4) t, generate_series(1,200) p,
       generate_series(1,100) c WHERE (t + p) % 2 = 0;
ANALYZE;
DELETE FROM ref_y WHERE tenant_id = 2 AND period = 75;   -- pair absent, both values common
```

**Mechanism, and it resolves the `pg_stat` warning in entry 9.** RI trigger queries are invisible to
`EXPLAIN`, but `auto_explain` prints them with their plans — no stats-flush timing to get wrong, no
`pg_stat_force_next_flush()`, so no PG 14 version gate:

```sql
LOAD 'auto_explain';
SET auto_explain.log_min_duration = 0;
SET auto_explain.log_nested_statements = on;   -- this is what exposes the RI query
SET auto_explain.log_analyze = on;
SET auto_explain.log_buffers = on;
SET client_min_messages = log;                 -- so the plans land in psql output
```

Entry numbers are stable identifiers — completed entries are deleted in place rather than
renumbered, so the cross-references between entries keep pointing at the right thing.
Entry 1 (PGM201's false "not a downtime risk" claim) was completed on 2026-09-01.
Entry 2 is **deferred** as of 2026-09-02 and is not part of the suggested order.
Entry 3 (PGM004 DETACH … CONCURRENTLY doc fix) was completed on 2026-09-02.
Entry 4 (PGM023 ATTACH/DETACH PARTITION are never combinable) was completed on 2026-09-02.
Entry 5 (DROP TABLE <parent> takes its partitions with it, no CASCADE needed) was completed
on 2026-09-02 (#221); start at entry 6.
Entry 7 (PGM501's ordered-prefix predicate) was completed on 2026-09-09: `has_covering_index`
is now `has_indexed_fk_column`, collapsed to "does any usable index contain at least one FK
column, in any position" (tier 1 and tier 2 turned out to need the same boolean check). The
tier-2 justification is backed by a real measurement, not just the scratchpad reasoning that
motivated it — see `verify/tests/fk/pgm501_fk_composite_expression_index.sql`, which confirms
a composite index with a leading plain column still drives an Index/Bitmap Heap Scan (not a
Seq Scan) even when the trailing column is an expression. Entry 6 is unrelated and still
open.
Entry 8 (PGM501 message accuracy, and the accepted false negative) was completed on
2026-09-09: the finding message now leads with "queries joining or filtering on these
columns" (the more frequent cost, and per the measured facts the larger one on partitioned
tables) before mentioning "referential integrity checks" — no longer "deletes/updates on
the referenced table", which implied any UPDATE triggers the check. Kept the message down
to a bare "referential integrity checks", no "on deletes or key updates" qualifier — that
phrase is self-explanatory at this length and the qualifier just added words. This message
lands verbatim in GitHub/SonarQube inline comments, so the fuller "more common cost" framing
and the deletes/key-update nuance live in EXPLAIN instead, not the message. The EXPLAIN text
and `docs/examples/pgm501_body.md` were brought in sync with entry 7's "any position" column
matching (both still described prefix-only matching, a gap entry 7 left behind) as well as
the new message wording. The accepted false negative (low-cardinality indexed FK column) is
now documented in EXPLAIN, SPEC.md, and the docs body — decided, not fixed, since index shape
alone can't distinguish it from a selective index.
Entry 9 (PGM501's verification test asserted a false claim) was completed on 2026-09-09.
Both false/untested claims in `verify/tests/fk/pgm501_fk_seqscan_without_index.sql` are fixed:
the prefix-matching claim now states that column order/position is irrelevant (idx(b,a) and
idx(a) both avoid Seq Scan too, matching entry 7's "any position" fix), with new assertions for
both shapes; the enforcement-path claim is split into a lookup-path claim (what the file's
`assert_explain_contains` calls actually test) and a real enforcement-path claim, the latter
verified by a new sidecar, `pgm501_fk_seqscan_without_index.check.sh`. The RI trigger's own
query is invisible to plain `EXPLAIN`; only `auto_explain` with `log_nested_statements` reveals
it, and it lands in the container's stderr rather than a query result, so the sidecar diffs
`docker logs` around a real `DELETE`, scoped to the block whose `Query Text` mentions the
referencing table (a fixed `grep -A N` was tried first and confirmed live to either truncate or
overrun into the next log block depending on N; the actual fix scopes by the tab-indented
continuation lines postgres's multi-line log format uses, stopping at the first non-tab line).
`verify/lib/runner.sh` now runs a sibling `<test>.check.sh`, if present, after the `.sql` file
and before collecting `_verify_results`, as a generic convention for assertions needing
shell-level orchestration rather than a plain SQL `assert_*` call — not hardcoded to this one
test. The `pg_stat`/`@min_version` gating idea from this entry's own notes was dropped entirely
per its own "probably moot" conclusion; nothing in the new work needs it.
New file `verify/tests/fk/pgm501_fk_partitioned.sql` encodes the partitioned-FK claims from the
facts section: the RI check traverses partitions rather than the always-empty parent; partition
key present in the FK prunes to exactly one partition while partition key absent locks every
partition regardless of an index on the FK columns (both proven by `pg_locks` counts inside an
uncommitted `DELETE`, not by plan text — note the count must be captured via `\gset` *before*
the enclosing `ROLLBACK`, since a plain `SELECT assert_eq(...)` inside that transaction has its
own `_verify_results` insert rolled back along with it); `ON DELETE CASCADE` fans out
identically; the referenced side carries one trigger pair regardless of partition count, unlike
`pg_constraint` which holds one row per partition plus the parent; a UNIQUE index on a
partitioned table cannot omit the partition key; and generic-plan pruning is visible at runtime
as `Subplans Removed` in `EXPLAIN ANALYZE` once `plan_cache_mode = force_generic_plan`. All 15
assertions across the three `fk/` test files pass on PG 14–18 with no version-specific behavior
found, so no `@min_version` gate was needed anywhere in the new file. The "ordinary joins" and
`assert_lock_blocks`/`assert_lock_allows` items from this entry's notes were out of scope and
left alone — the former is general FK-index behavior already covered by
`pgm501_fk_seqscan_without_index.sql`, and the latter was already removed from
`verify/lib/framework.sql` before this entry started.
Entry 10 (PGM024: the actual rule) was completed on 2026-09-10. PGM024 fires on `DROP TABLE`
of a live partition child and on `CREATE TABLE ... PARTITION OF` a pre-existing parent, both
Critical severity, dedup'd by the parent's catalog key so one migration touching many children
of the same parent produces one finding per parent rather than one per statement. It carries a
same-unit DETACH-then-DROP guard so the safe pattern PGM004 recommends (`DETACH PARTITION ...
CONCURRENTLY` followed by `DROP TABLE` in the same file) does not also trip PGM024. A dedicated
fixture repo, `tests/fixtures/repos/partition-lock-hazard/`, reproduces the original real
migration that used to produce 62 findings (PGM201×20, PGM401×20, PGM402×14, PGM501×4,
PGM502×2, PGM508×2) with none about locks, the parent, or blocking.

## 2. PGM201: name the parent in the finding message — DEFERRED

**Deferred 2026-09-02.** Written in full, reviewed, and reset. It worked and the whole suite was green, but it
emits a **false** message on the one shape PGM201's own `EXPLAIN` tells you to use, and the cause is
architectural rather than local — a rule cannot see catalog state produced earlier in its own migration unit.
Deferred to that rework. **Skip this entry in the top-to-bottom order.**

### The plan, unchanged

- When `catalog_before` records the dropped table as a partition child, append to the message: it is a partition of `<parent>`, the drop takes `AccessExclusive` on the parent, detach first. When `parent_table` is `None` the message must stay **byte-identical** to today's.
- Helper: `parent_display_name(ctx, table_key)` reading `ctx.catalog_before.get_table(key)?.parent_table`, resolving the parent's `display_name` and falling back to the parent's catalog key when the parent itself is not in the catalog. Same idiom as `src/rules/pgm002.rs` / `src/rules/pgm501.rs`.
- Tests: partition-child message, standalone message unchanged, and untracked-parent falls back to the key.
- Do **not** change severity or what it fires on — PGM024 (entry 10) owns the Critical lock hazard.
- `SPEC.md` documents one **Message** bullet; split it into "standalone table, or parent unknown" (verbatim old text) and "known partition child", the way PGM202/PGM205 already document conditional messages.

### What was built, so the redo is mechanical

- The helper landed as a **method on `LintContext`** (`src/rules/lint_context.rs`), not a free function:
  `ctx.parent_display_name(key)` reads the same as the planned signature, and entry 10 (PGM024) needs the
  identical lookup. Body is `catalog_before.get_table(table_key)?.parent_table.as_ref()?`, then the parent's
  `display_name` when the parent is tracked and `parent_key.clone()` when it is not.
- `pgm201::check` builds the base message into a `let mut message = format!(...)` and appends only when the
  helper returns `Some`, which is what keeps the standalone message byte-identical. The text that shipped,
  verbatim:
  `" It is a partition of '{parent}': the drop takes ACCESS EXCLUSIVE on '{parent}' and holds it until commit, blocking reads and writes routed through the parent. DETACH PARTITION ... CONCURRENTLY first, then drop the standalone table."`
- Three tests as specified, plus a **non-snapshot** `assert_eq!` on the exact standalone message — the
  snapshot alone cannot guard byte-identity, because an accepted snapshot silently redefines it.
- `SPEC.md`: the two Message bullets, plus a sub-bullet stating that `{parent}` is the parent's
  `display_name` when tracked and its catalog key otherwise, and that "parent unknown" means
  `catalog_before` records no `parent_table` **at all** — not merely that the parent itself is untracked.
- Green on `cargo test` (898 lib tests + every integration suite), `cargo test --features docgen`, and
  `cargo clippy --all-targets --features bridge-tests -- -D warnings`. `EXPLAIN` was untouched, so **no
  docgen/`make docs-sync` pass is needed** for this entry.
- **No fixture repo under `tests/fixtures/repos/` drops a partition child**, so not one integration snapshot
  moved. Do not budget for a snapshot ripple here (unlike entry 7).

### Why it is deferred: the append is blind to its own file

`parent_display_name` reads `catalog_before`, and `src/pipeline.rs:50` clones that **before `replay::apply`
processes the whole unit**. `apply_alter_table` in `src/catalog/replay.rs` is what clears `parent_table` on
`DetachPartition` (and sets it on `AttachPartition`), so those writes only ever land in the post-unit catalog.
A rule cannot see a detach three lines above the statement it is judging.

`catalog_before` is still the only usable catalog here — in `catalog_after` the dropped table is gone, so the
parent could never be recovered. The blind spot is not a wrong choice of catalog. This is the same
architectural wall as the deferred file/transaction-level variant in entry 10.

**Reproduced against a binary built from the change**, not reasoned about, with `run_in_transaction = false`
so the concurrent detach is legal:

```sql
-- V001: measurements PARTITION BY RANGE (ts), child measurements_2023 PARTITION OF measurements
-- V002, the changed file:
ALTER TABLE measurements DETACH PARTITION measurements_2023 CONCURRENTLY;
DROP TABLE measurements_2023;
```

emits `It is a partition of 'measurements': the drop takes ACCESS EXCLUSIVE on 'measurements' and holds it
until commit … DETACH PARTITION ... CONCURRENTLY first, then drop the standalone table.` After a
**successful** concurrent detach the child is a standalone table: the drop takes no lock on the parent, and
the prescribed fix is the line above it. The author who followed step 4 of PGM201's own `EXPLAIN` is told to
follow it again.

Scope of the defect, established the same way — do not re-derive:

- **Only the `CONCURRENTLY` variant is false.** With a plain `DETACH PARTITION` in the same transaction the
  detach itself holds `AccessExclusive` on the parent until commit, so the appended claim is true of the
  migration and "use the CONCURRENTLY form" is correct, actionable advice. Suppressing there deletes a true
  warning, and PGM004 already fires Critical on that line.
- Same-unit `ATTACH PARTITION` then `DROP TABLE <child>` gets **no** partition text at all — false negative.
- Same-unit `ALTER TABLE parent RENAME TO …` then `DROP TABLE <child>` names the **pre-rename** parent.
- Those last two are cosmetic: neither asserts a lock that will not be taken.
- The `PARTITION OF` and `ATTACH PARTITION` paths in *earlier* units both resolve correctly. There is no
  false negative there.

And the wrinkle that stopped any local fix from being obviously right: after an **interrupted**
`DETACH … CONCURRENTLY` the child is still in `pg_inherits` with `inhdetachpending`, and its `DROP TABLE`
**does** still take `AccessExclusive` on the parent (measured — see the facts section). So the appended
sentence is false on the happy path and true on the interrupted path. Blanket suppression trades a false
positive for a lost warning on the nastier shape.

Three options were on the table and **none was chosen**:

1. Suppress the append when the same unit detaches that child first. Order-insensitive is fine —
   `DROP TABLE <child>` before its own detach is not valid SQL, so it cannot occur in a working migration.
   Minimal, one predicate.
2. Suppress only when `concurrent: true`. Strictly more accurate; keeps the true plain-detach warning.
3. A third message variant for the same-unit-detach case, naming the `inhdetachpending` risk instead of the
   parent lock. Most honest, needs its own `SPEC.md` bullet, and drifts into entry 3's territory.

If a local fix is ever preferred to the rework: the in-repo precedent for within-unit cross-statement state is
`src/rules/pgm023.rs:53` (`HashMap<(String, LockLevel), SourceSpan>` carried across statements), **not**
`src/rules/pgm022.rs`, which is a plain per-statement loop. Note too that `check_existing_table`'s closure
(`src/rules/existing_table_check.rs`) receives no statement index, so anything order-sensitive needs that
signature changed — which ripples to all six rules sharing it.

### Two secondary findings, worth folding into the redo

- **No test in the repo can distinguish the two branches of `parent_display_name`.**
  `TableBuilder::new(name)` sets `display_name == name` (`src/catalog/builder.rs`), so every test has them
  identical and the tracked-parent branch would pass just as happily if it returned the catalog key. In
  production they differ — the key is schema-qualified, `display_name` is what the author wrote — so the
  message would read `public.measurements` instead of `measurements` and nothing would catch it. Needs a
  `display_name` setter on `TableBuilder` plus one test where the two differ. The same blind spot already
  covers every other rule that prints a `display_name`, `src/rules/pgm002.rs` among them.
- **Accept snapshots with `cargo insta accept`, not `mv`.** `mv`-ing a `.snap.new` keeps its
  `assertion_line:` header, which `accept` strips. Only one of ~194 snapshots in the repo carries one today
  (a pre-existing PGM005 file), and it churns the diff whenever the test moves lines.

### Reviewed and rejected — do not re-raise

Four review dimensions produced 10 findings; 9 died under adversarial verification. Recorded so they are not
re-derived:

- *The absent-parent fallback turns a stale `parent_table` into a confident `AccessExclusive` claim about a
  table the catalog no longer has.* The staleness is entry 5's subject, and the fallback names an existing
  key rather than inventing one.
- *SARIF `rules[].shortDescription` for PGM201 becomes a 372-char four-sentence paragraph.* Confuses the
  finding **message** with `DESCRIPTION`. Nothing in `src/output/` puts finding messages into rule metadata,
  and neither SARIF nor the SonarQube importer imposes a message length limit that this text approaches.
- *No negative test for a child that was a partition and is not one any more.* That is the confirmed defect
  above, not a separate coverage gap.
- *`test_drop_table_created_in_same_change_no_finding` passes for the wrong reason, duplicating the
  nonexistent-table test.* Pre-existing test, unrelated to this entry — and it does pin a distinct branch
  (`tables_created_in_change` vs. absent from `catalog_before`).
- The remaining five were duplicates: one more report of the same-unit detach, three more of the
  `assertion_line` header, and the `display_name` branch gap. The last two are the secondary findings above,
  kept despite the verdict because both are one-line fixes.

## 6. Stale documentation

- Ground truth from `src/rules/rule_id.rs`: 0xx = PGM001–023 (23), 1xx = 101–109 (9), 2xx = 201–205 (5), 3xx = 301–303 (3), 4xx = 401–403 (3), 5xx = 501–509 (9) → **52 lint rules** + PGM901 meta = 53 `RuleId` variants. `src/docgen.rs` computes `rule_count` excluding PGM901, so "52 lint rules" is the project's established wording.
- `README.md`: make "52 rules" read "52 lint rules"; `PGM001-PGM022` → `PGM001-PGM023`; the bare "Critical/Major" claim for 0xx needs softening ("mostly") since PGM023/006/010/012/020 are Minor and PGM009 is Info.
- `docs/rules.md.j2`: **four** hardcoded family ranges are wrong, not three — `PGM001–PGM020`→`023`, `PGM101–PGM106`→`109`, `PGM201–PGM204`→`205`, `PGM501–PGM506`→`509`. Deriving them from `families` would need a `FamilyContext` change (it carries only `heading` and a 1xx-only `intro`), so keep them hardcoded.
- `CONTRIBUTING.md`: the "Adding a rule" section documents two constants and a four-match-arm dispatch; the code wants **three** constants (`DESCRIPTION`, `EXPLAIN`, `DEFAULT_SEVERITY`) plus one `dispatch_rules!` mapping. Following it literally does not compile. Also its claim that a rule's `EXPLAIN` self-reference "is verified by tests" is false — `src/rules/mod.rs` only checks lengths; soften to a review convention or add the test.
- `SPEC.md` points at `docs/dont-do-this-rules.md`, which does not exist; repoint or remove.
- `CLAUDE.md` rule list and counts (already partly corrected in a previous pass — re-check against `rule_id.rs`).

## 10. PGM024: the actual rule (the original goal)

The linter did **not** catch the partition lock hazard until PGM024 was added: on the real migration it used to emit 62 findings
(PGM201×20, PGM401×20, PGM402×14, PGM501×4, PGM502×2, PGM508×2) and **not one** mentions locks, the
parent, or blocking. It exits 0 at the default `fail_on = "critical"`. Coverage is inverted: the same
intent written as `DETACH PARTITION` fires **CRITICAL PGM004** with a correct explanation, while the
strictly worse `DROP TABLE <child>` — same parent lock, plus irreversible data loss, plus no concurrent
variant — is MINOR.

- **No IR or catalog work is needed.** `CreateTable.partition_of` already carries the parent
  (`src/parser/pg_query.rs` reads `create.partbound` / `inh_relations[0]`), replay writes
  `TableState.parent_table`, and PGM501 already reads it in production code. Rule (a) is ~30 lines:
  match `IrNode::DropTable`, `ctx.catalog_before.get_table(key)`, read `.parent_table`.
- Fire on both `DROP TABLE <child>` and `CREATE TABLE <child> PARTITION OF <parent>` where the parent is
  pre-existing. Severity **Critical** — anything less keeps the default CI gate green on this shape.
- Set `dedup_key` to the **parent's** key so 192 drops across 12 parents give 12 findings, not 192. See
  the finding-bundling entry below.
- Next free ids: **PGM025** / 110 / 206 / 304 / 404 / 510 / 902. SPEC.md no longer has a proposed
  4-digit rules section (the last, PGM1403, was promoted in v1.13).
- Registration checklist, code-verified: new `src/rules/pgmXXX.rs` with the three `pub(super)` constants
  and `check`; `mod pgmXXX;` in `src/rules/mod.rs`; `RuleId` variant plus one `dispatch_rules!` mapping in
  `src/rules/rule_id.rs`; two exhaustive SonarQube matches; `docs/examples/pgmXXX_body.md`; three snapshot
  families; two fixture repos; and one hardcoded count. README/`docs/index.md` are **not** auto-generated
  and no test guards them.
- **No partition fixture repo exists.** `tests/fixtures/repos/` has nothing partition-centric. A faithful
  5-parent reduction of the real migration is a ready-made fixture, and it has been **rescued out of the
  session scratchpad** — it is now `001_init.sql` / `002_repartition.sql` in the **repo root**, untracked as
  of 2026-09-01 (`001` creates 5 range-partitioned parents plus their 20 quarterly 2027 partitions; `002` is
  the drop-and-recreate wrapped in `BEGIN`/`COMMIT`). Move them into a real fixture repo under
  `tests/fixtures/repos/` rather than leaving them at the root. Fallback if they go missing again: rebuild
  from the setup SQL in `verify/lock-tests/tests/partition_drop_blocked_by_traffic.rs`.
- Deferred, needs design: a **file/transaction-level** variant reporting how many distinct parents one
  file locks. Three gaps block it — `BEGIN;`/`COMMIT;` collapse to `IrNode::Ignored` and
  `run_in_transaction` is a per-repo config default, so in-file transaction boundaries are invisible;
  there is no cross-file aggregation (`rule.check` runs once per `MigrationUnit`); and every `Finding`
  needs a `SourceSpan`, so there is no file-level finding to anchor to.

## 11. PGM003 does not cover DETACH … CONCURRENTLY

- PGM003 flags `CONCURRENTLY` inside a transaction block but only inspects `CreateIndex` and `DropIndex`
  (`src/rules/pgm003.rs`). `DETACH PARTITION … CONCURRENTLY` also cannot run in a transaction block, and
  it is the fix PGM004 and PGM201 now recommend — so the tool recommends a fix whose main failure mode it
  cannot detect. Add `AlterTableAction::DetachPartition { concurrent: true, .. }`.
- Fold in with the existing REINDEX gap below; same rule, same match arm.

## 12. rust-analyzer cannot see verify/lock-tests

- `verify/lock-tests` is deliberately outside the root workspace, so rust-analyzer never discovers it and
  every file in it has no go-to-definition. `rust-analyzer.linkedProjects` with both manifests fixes the
  Rust side, but the requirement is a solution covering **the whole project** (the `bridge/` Maven module
  too), so a two-manifest `.vscode/settings.json` was rejected. Unresolved.

## Candidate rule: FK on a partitioned table that omits the partition key (PGM510?)

**Not decided — this is a candidate, written up so the measurement is not lost.** It came out of the
partitioned-FK facts above and is a schema-design smell (5xx family), not a lock hazard.

- The shape: referencing table is partitioned, and the FK column list does **not** include the partition
  key. Every delete of a referenced row, and every non-nested-loop join on those columns, then touches
  every partition — `Append` over all of them plus `RowShareLock` on each, linear in partition count. A
  covering index does not help; there is nothing to prune on. Adding the partition key to the FK is the fix,
  and it is a schema decision that is very expensive to change later, which is exactly the kind of thing
  worth flagging at migration time.
- **The catalog already has everything needed.** `TableState.partition_by` is
  `Option<PartitionByInfo { strategy: PartitionStrategy, columns: Vec<String> }>`
  (`src/catalog/types.rs:109`), so the check is a set-containment test between `columns` and the FK's column
  list. Same "no IR or catalog work needed" situation as entry 10.
- Open questions before this is worth writing:
  1. Does it fire on the parent's FK, on each partition's cloned FK, or once per parent? `dedup_key` on the
     parent's key, as entry 10 does, is probably right.
  2. Severity. The cost is real but proportional to partition count, which the catalog does know
     (partitions are tracked via `parent_table`), so a threshold is possible. Major seems the ceiling —
     this is not a downtime risk, it is a design cost.
  3. **The LIST carve-out is not decidable from the catalog.** `PartitionStrategy` distinguishes
     List/Range/Hash, but nothing records per-partition *bounds*, so a single-value LIST partition — the one
     shape where the partition key genuinely adds nothing to an index — is indistinguishable from a
     multi-value one. Affects the wording of any advice about indexes, not the fire condition.
  4. Overlap with PGM501: the two would fire together on a partitioned table with neither the partition key
     in the FK nor an index. Decide whether that is one finding or two.
- If this is **not** adopted, the partitioned-FK facts above should still land in PGM501's `EXPLAIN` as
  context, since PGM501 is the rule users will be reading when they hit this.

## EXCLUDE constraint catalog accuracy

- **`ConstraintState::Exclude` does not track columns.** The IR's `TableConstraint::Exclude` only captures the constraint name, not the element list (columns and operators). This means `involves_column()` always returns `false` for EXCLUDE constraints, and `remove_column()` never drops them — diverging from PostgreSQL, which drops EXCLUDE constraints when a referenced column is dropped. Fix: expand `TableConstraint::Exclude` to capture element column names, then wire `involves_column()` and `remove_column()` to use them. See TODO comments in `src/catalog/types.rs` and `src/catalog/replay.rs`.
type name is string not type for some kind of check?

## UNIQUE constraints register no index in the catalog

- **`ALTER TABLE ... ADD UNIQUE` records a `ConstraintState::Unique` but never pushes an `IndexState`.** `apply_table_constraint` in `src/catalog/replay.rs` creates a synthetic `<table>_pkey` index for `PRIMARY KEY` (both the inline-column and table-level paths route through it), but the `Unique` arm around `src/catalog/replay.rs:783` pushes only the constraint. PostgreSQL always backs a UNIQUE constraint with a unique index, so the catalog is missing an index that really exists.
- Consequence: any rule that reasons over `TableState::indexes` under-counts. Confirmed relevant to the FK covering-index check; a foreign key whose columns are covered by a UNIQUE *constraint* (rather than an explicit `CREATE UNIQUE INDEX`) is a false positive.
- Fix is small — mirror the PK arm — but the ripple is not, so it needs its own change:
  1. **PGM508 (redundant index)** would start seeing a new index and could begin reporting an explicit index as redundant with a UNIQUE constraint's implicit one. That may be correct and useful, or noise; decide deliberately.
  2. **PGM503 (UNIQUE NOT NULL instead of PK)** already checks constraints *and* indexes separately in `has_unique_not_null` — adding the synthetic index could double-count or change which branch fires.
  3. Naming: PostgreSQL's generated name is `<table>_<columns>_key`, not `<table>_pkey`-style. Getting it wrong makes a later `DROP INDEX` in the migration history fail to match.
  4. `ADD UNIQUE USING INDEX` must not create a second index, same carve-out the PK arm already has.
- Also unresolved and adjacent: the covering-index predicate is btree-only. A HASH index supports equality on its single column and would keep an FK lookup off a sequential scan, so btree-only is over-strict. Left alone deliberately.

## Bundle repeated findings instead of emitting one per statement

- **A rule that fires on every statement in a large file drowns out everything else.** The real 192-`DROP TABLE` partition migration produced an idempotency warning (missing `IF EXISTS`, 4xx family) on *every* table — ~192 identical findings saying the same thing, which is pure noise and buries any finding that actually matters. Worse, the thing that mattered in that migration (every one of those drops takes ACCESS EXCLUSIVE on the partition's parent) was not reported at all, so the signal-to-noise ratio was zero over ~192 lines.
- Wanted: when one rule fires many times in one file, emit **one aggregated finding** — ideally surfaced at the top — rather than N identical ones. Something like "PGM401: 192 statements in this file drop objects without `IF EXISTS`" anchored at the first occurrence.
- Design questions to settle before implementing:
  1. Where does bundling live — in the rule, in the engine after `check()` returns, or in the reporter? Engine-level is the only place that can apply a uniform policy without touching all 52 rules.
  2. What is the trigger — a fixed threshold (>N of the same rule in one file), or always collapse per (rule, file)?
  3. Keep the individual locations? SARIF supports multiple `locations` on one result, which would give one alert with 192 locations — probably the right shape for GitHub Code Scanning, which otherwise opens 192 alerts.
  4. SonarQube Generic Issue Import wants one issue per location and has no equivalent grouping, so the two output formats may need different collapsing behaviour.
  5. Does the aggregated finding keep the rule's severity, and does a suppression comment on one statement suppress the whole bundle?
- Verify the exact rule id and count against the run in `tests/fixtures/` before writing the issue up; the "idempotency warning on every table" observation is from the production migration, not yet from a fixture.

## PGM003 does not detect REINDEX CONCURRENTLY inside transaction

- **`pgm003::check` only matches `CreateIndex` and `DropIndex`.** The match arm at `src/rules/pgm003.rs:50-53` does not handle `IrNode::Reindex`, so `REINDEX CONCURRENTLY` inside a transaction silently passes. When implementing:
  1. Add match arm `IrNode::Reindex(r) => r.concurrent` in `src/rules/pgm003.rs`
  2. Add test `test_reindex_concurrent_in_transaction_fires`
  3. Update module doc comment, `EXPLAIN` text, and `DESCRIPTION` in `pgm003.rs` to mention REINDEX CONCURRENTLY
  4. Update `docs/examples/pgm003_body.md` to mention REINDEX CONCURRENTLY
  5. Add PGM022 cross-reference to PGM003's "See also" (both explain text and body markdown)
  6. Add PGM003 cross-reference to PGM022's explain text and `docs/examples/pgm022_body.md`

## Land a single source of truth for rule texts

- **Every rule's prose is written five times, by hand, with nothing keeping the copies in agreement.** For one rule that is: the `//!` module doc, the `DESCRIPTION` constant, the `EXPLAIN` constant, `docs/examples/pgmXXX_body.md`, and the rule's section in `SPEC.md`. Only two of the five are generated from anything — `docs/rules.md` is `sed`'d out of the docgen snapshot by `make docs-sync`, and the `--explain` snapshot is captured from `EXPLAIN`. The five sources themselves are independent.
- Observed while correcting PGM201's false "not a downtime risk" claim, all in one sitting: it had to be corrected in three places separately; the `EXPLAIN` and the body markdown drifted apart on punctuation within minutes of each other; and `SPEC.md`'s **Why** bullet ended up thinner than both, omitting the queued-lock mechanism and the DETACH caveat the other two carried. `SPEC.md` is the designated source of truth per `CLAUDE.md`, so it is the copy that most needs to not be a copy.
- Nothing catches drift. `src/rules/mod.rs` only asserts that `DESCRIPTION`/`EXPLAIN` are non-empty and within a length bound; no test compares any two of the five, and no test reads `SPEC.md` at all.
- Secondary irritant: `EXPLAIN` is a `\n\`-continued string literal, so a line missing its trailing `\` silently swallows the newline **and** the next line's nine spaces of indentation into the string. Hit once on PGM201; `cargo fmt` and `clippy` are both clean when it happens, and it only shows up by eye in `--explain` output.
- Design questions to settle first:
  1. Which copy becomes canonical? `docs/examples/pgmXXX_body.md` is the only one already in markdown and already read at build time, so it is the natural candidate — but `EXPLAIN` is plain text for a terminal and `SPEC.md` wants a different register (behaviour spec, not user advice). One source rendering three ways is a real templating job, not a `include_str!`.
  2. Or keep the copies and add a **consistency test** instead — far cheaper, catches drift rather than preventing it. Would need a machine-checkable relation between the texts; "SPEC mentions every SQLSTATE the EXPLAIN mentions" is checkable, "says the same thing" is not.
  3. If `EXPLAIN` stays hand-written, replace the `\n\` continuation style with `concat!` or an `include_str!` of a plain-text file to make the missing-backslash bug unrepresentable.
  4. Scope: 52 rules × 5 texts. Any migration here is mechanical but large, and it collides with every other entry in this file that edits rule prose — so decide before doing entries 3, 6, 8 and 10, not after.
