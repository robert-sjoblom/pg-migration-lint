-- @claim: Generated names are cut to NAMEDATALEN-1 = 63 bytes: {name1}_{name2}_{label}, label never cut
-- @claim: The byte budget for name1 + name2 is 63 - len(label) - 1, minus 1 more when there is a name2
-- @claim: While over budget, the longer of name1/name2 loses one byte; on a tie name2 loses it
-- @claim: Each part is then clipped back to a whole character, so a multibyte name can end up under 63 bytes
-- @claim: A multi-column name2 is the joined column list, cut as one string
-- @claim: A collision suffix goes on the label (key1), so the parts are re-cut to a budget one byte smaller
-- @min_version: 14

SET client_encoding = 'UTF8';
CREATE SCHEMA app;
SET search_path = app, public;

-- name1 only (pkey): budget 63 - 4 - 1 = 58
CREATE TABLE p23456789012345678901234567890123456789012345678901234567890123 (id int PRIMARY KEY);
SELECT assert_eq(
    constraint_names('app.p23456789012345678901234567890123456789012345678901234567890123', 'p'),
    'p234567890123456789012345678901234567890123456789012345678_pkey',
    '63-char table: PK keeps 58 chars of the table name + _pkey');

-- long table, short column: only the table is cut; budget 63 - 3 - 2 = 58, a keeps its 1
CREATE TABLE u23456789012345678901234567890123456789012345678901234567890123 (a int UNIQUE);
SELECT assert_eq(
    constraint_names('app.u23456789012345678901234567890123456789012345678901234567890123', 'u'),
    'u23456789012345678901234567890123456789012345678901234567_a_key',
    '63-char table, column a: UNIQUE keeps 57 chars of the table name');

CREATE TABLE k23456789012345678901234567890123456789012345678901234567890123 (b int CHECK (b > 0));
SELECT assert_eq(
    constraint_names('app.k23456789012345678901234567890123456789012345678901234567890123', 'c'),
    'k234567890123456789012345678901234567890123456789012345_b_check',
    '63-char table, column b: CHECK keeps 55 chars of the table name (longer label, smaller budget)');

-- short table, long column: only the column is cut
CREATE TABLE short (c23456789012345678901234567890123456789012345678901234567890123 int UNIQUE);
SELECT assert_eq(constraint_names('app.short', 'u'),
    'short_c2345678901234567890123456789012345678901234567890123_key',
    'short table, 63-char column: the column keeps 53 chars, the table is intact');

-- 50 + 11 = 61 > 58: the longer part (the table) gives up all 3 bytes
CREATE TABLE w2345678901234567890123456789012345678901234567890 (customer_id int UNIQUE);
SELECT assert_eq(constraint_names('app.w2345678901234567890123456789012345678901234567890', 'u'),
    'w2345678901234567890123456789012345678901234567_customer_id_key',
    '50-char table, 11-char column: only the longer table name is cut, to 47');

-- both 63: trimmed alternately down to 29 + 29 = 58
CREATE TABLE x23456789012345678901234567890123456789012345678901234567890123 (
    y23456789012345678901234567890123456789012345678901234567890123 int UNIQUE);
SELECT assert_eq(
    constraint_names('app.x23456789012345678901234567890123456789012345678901234567890123', 'u'),
    'x2345678901234567890123456789_y2345678901234567890123456789_key',
    'both 63 chars: each part is cut to 29');

-- 30 + 30 = 60 into fkey budget 57: name2, name1, name2 lose a byte, giving 29 + 28
CREATE TABLE ref (id int PRIMARY KEY);
CREATE TABLE q23456789012345678901234567890 (r23456789012345678901234567890 int REFERENCES ref);
SELECT assert_eq(constraint_names('app.q23456789012345678901234567890', 'f'),
    'q2345678901234567890123456789_r234567890123456789012345678_fkey',
    'equal-length parts: the tie goes against name2, giving table 29 and column 28');

-- multi-column index: name2 = a..._b..._c... (92 chars), cut as one string to 56
CREATE TABLE mc (
    a23456789012345678901234567890 int,
    b23456789012345678901234567890 int,
    c23456789012345678901234567890 int);
CREATE INDEX ON mc (a23456789012345678901234567890, b23456789012345678901234567890, c23456789012345678901234567890);
SELECT assert_eq(index_names('app.mc'),
    'app.mc_a23456789012345678901234567890_b234567890123456789012345_idx',
    'multi-column index: the joined column list is cut as one string, the third column vanishes');

-- multibyte: 'a' + 31 x 'å' is 63 bytes. Budget 58 lands inside the 29th 'å', so it clips to 57 bytes.
CREATE TABLE "aååååååååååååååååååååååååååååååå" (id int PRIMARY KEY);
SELECT assert_eq(constraint_names('app."aååååååååååååååååååååååååååååååå"', 'p'),
    'aåååååååååååååååååååååååååååå_pkey',
    'multibyte table name is clipped to a character boundary: a + 28 x å + _pkey');
SELECT assert_eq(octet_length(constraint_names('app."aååååååååååååååååååååååååååååååå"', 'p'))::bigint, 62::bigint,
    'multibyte clip leaves the PK name at 62 bytes, not 63');

-- multibyte column: 'b' + 31 x 'ö' is 63 bytes, cut to 56 bytes lands inside the 28th 'ö', clips to 55
CREATE TABLE tm ("bööööööööööööööööööööööööööööööö" int UNIQUE);
SELECT assert_eq(constraint_names('app.tm', 'u'),
    'tm_bööööööööööööööööööööööööööö_key',
    'multibyte column name is clipped to a character boundary: b + 27 x ö');
SELECT assert_eq(octet_length(constraint_names('app.tm', 'u'))::bigint, 62::bigint,
    'multibyte clip leaves the UNIQUE name at 62 bytes');

-- collision suffix inside the budget
CREATE TABLE d23456789012345678901234567890123456789012345678901234567890123 (a int);
ALTER TABLE d23456789012345678901234567890123456789012345678901234567890123 ADD UNIQUE (a);
ALTER TABLE d23456789012345678901234567890123456789012345678901234567890123 ADD UNIQUE (a);
SELECT assert_eq(
    constraint_names('app.d23456789012345678901234567890123456789012345678901234567890123', 'u'),
    'd23456789012345678901234567890123456789012345678901234567_a_key,d2345678901234567890123456789012345678901234567890123456_a_key1',
    'second UNIQUE on a 63-char table: label key1, table cut to 56 instead of 57');
CREATE INDEX ON d23456789012345678901234567890123456789012345678901234567890123 (a);
CREATE INDEX ON d23456789012345678901234567890123456789012345678901234567890123 (a);
SELECT assert_eq(
    (SELECT string_agg(i.relname, ',' ORDER BY i.oid) FROM pg_index x JOIN pg_class i ON i.oid = x.indexrelid
      WHERE x.indrelid = to_regclass('app.d23456789012345678901234567890123456789012345678901234567890123')
        AND NOT EXISTS (SELECT 1 FROM pg_constraint k WHERE k.conindid = i.oid)),
    'd23456789012345678901234567890123456789012345678901234567_a_idx,d2345678901234567890123456789012345678901234567890123456_a_idx1',
    'second CREATE INDEX on a 63-char table: label idx1, table cut to 56');

-- two tables that differ only after byte 58 truncate to the same base and collide
CREATE TABLE z23456789012345678901234567890123456789012345678901234567890aaa (id int PRIMARY KEY);
CREATE TABLE z23456789012345678901234567890123456789012345678901234567890bbb (id int PRIMARY KEY);
SELECT assert_eq(
    constraint_names('app.z23456789012345678901234567890123456789012345678901234567890aaa', 'p'),
    'z234567890123456789012345678901234567890123456789012345678_pkey',
    'first of two long tables with a shared 58-char prefix gets the plain truncated name');
SELECT assert_eq(
    constraint_names('app.z23456789012345678901234567890123456789012345678901234567890bbb', 'p'),
    'z23456789012345678901234567890123456789012345678901234567_pkey1',
    'second one collides after truncation: pkey1, table cut to 57');
