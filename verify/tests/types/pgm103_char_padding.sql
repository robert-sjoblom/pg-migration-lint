-- @claim: char(n) pads with spaces, wastes storage (PGM103)
-- @claim: No performance difference among char(n), varchar(n), and text
-- @min_version: 14

-- Test 1: char(n) pads with spaces
CREATE TABLE test_char(
    c char(10),
    v varchar(10),
    t text
);

INSERT INTO test_char VALUES ('hello', 'hello', 'hello');

-- char(10) stores 'hello     ' (padded to 10)
SELECT assert_eq(
    (SELECT length(c)::bigint FROM test_char),
    5::bigint,
    'char(10) length() returns 5 (strips trailing spaces for comparison)'
);

-- But octet_length reveals the padding
SELECT assert_eq(
    (SELECT octet_length(c)::bigint FROM test_char),
    10::bigint,
    'char(10) octet_length is 10 (padded with spaces)'
);

-- varchar has no padding
SELECT assert_eq(
    (SELECT octet_length(v)::bigint FROM test_char),
    5::bigint,
    'varchar(10) octet_length is 5 (no padding)'
);

-- text has no padding
SELECT assert_eq(
    (SELECT octet_length(t)::bigint FROM test_char),
    5::bigint,
    'text octet_length is 5 (no padding)'
);

-- Test 2: char comparison semantics — trailing spaces are ignored
SELECT assert_true(
    (SELECT c = 'hello' FROM test_char),
    'char(10) equals string without trailing spaces'
);

SELECT assert_true(
    (SELECT c = 'hello     ' FROM test_char),
    'char(10) equals string with explicit trailing spaces'
);

-- Test 3: varchar and text do NOT ignore trailing spaces
SELECT assert_true(
    (SELECT v = 'hello' FROM test_char),
    'varchar matches without spaces'
);

SELECT assert_false(
    (SELECT v = 'hello     ' FROM test_char),
    'varchar does NOT match with trailing spaces'
);

-- Cleanup
DROP TABLE test_char;
