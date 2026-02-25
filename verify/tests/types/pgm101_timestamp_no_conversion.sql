-- @claim: timestamp without time zone stores no timezone info (PGM101)
-- @claim: timestamptz converts to UTC; timestamp does not
-- @min_version: 14

-- Test 1: timestamp ignores session timezone — same value regardless of TZ
SET timezone = 'America/New_York';
CREATE TABLE test_ts(id int, ts timestamp, tstz timestamptz);
INSERT INTO test_ts VALUES (1, '2024-06-15 12:00:00', '2024-06-15 12:00:00');

-- Change timezone and read back
SET timezone = 'Europe/Stockholm';

-- timestamp should return the SAME literal (no conversion)
SELECT assert_eq(
    (SELECT ts::text FROM test_ts WHERE id = 1),
    '2024-06-15 12:00:00',
    'timestamp returns same literal regardless of timezone'
);

-- timestamptz should return a DIFFERENT representation (converted from NY to Stockholm)
SELECT assert_true(
    (SELECT tstz::text FROM test_ts WHERE id = 1) != '2024-06-15 12:00:00',
    'timestamptz adjusts display to session timezone'
);

-- Test 2: Demonstrate the ambiguity problem — inserting "the same time" in different TZs
SET timezone = 'America/New_York';
INSERT INTO test_ts VALUES (2, '2024-06-15 12:00:00', '2024-06-15 12:00:00');

SET timezone = 'Europe/Stockholm';
INSERT INTO test_ts VALUES (3, '2024-06-15 12:00:00', '2024-06-15 12:00:00');

-- The timestamp columns will look the same but represent different actual moments
SELECT assert_eq(
    (SELECT ts::text FROM test_ts WHERE id = 2),
    (SELECT ts::text FROM test_ts WHERE id = 3),
    'timestamp values look identical despite different TZ at insert (ambiguous!)'
);

-- The timestamptz columns should differ (they represent different UTC moments)
SELECT assert_true(
    (SELECT tstz FROM test_ts WHERE id = 2) != (SELECT tstz FROM test_ts WHERE id = 3),
    'timestamptz values differ because they were inserted in different TZs (correct!)'
);

-- Cleanup
DROP TABLE test_ts;
RESET timezone;
