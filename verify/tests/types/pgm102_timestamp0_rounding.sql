-- @claim: timestamp(0) rounds (not truncates) fractional seconds (PGM102)
-- @claim: '23:59:59.9' rounds to next day
-- @min_version: 14

-- Test 1: Classic example — 0.9 seconds rounds UP, crossing day boundary
SELECT assert_eq(
    ('2024-12-31 23:59:59.9'::timestamp(0))::text,
    '2025-01-01 00:00:00',
    'timestamp(0) rounds 23:59:59.9 to next day (not truncates to 23:59:59)'
);

-- Test 2: 0.5 rounds up
SELECT assert_eq(
    ('2024-06-15 12:00:00.5'::timestamp(0))::text,
    '2024-06-15 12:00:01',
    'timestamp(0) rounds 0.5 UP to next second'
);

-- Test 3: 0.4 rounds down (truncates)
SELECT assert_eq(
    ('2024-06-15 12:00:00.4'::timestamp(0))::text,
    '2024-06-15 12:00:00',
    'timestamp(0) rounds 0.4 DOWN (stays at same second)'
);

-- Test 4: timestamptz(0) also rounds
SELECT assert_eq(
    ('2024-12-31 23:59:59.9+00'::timestamptz(0) AT TIME ZONE 'UTC')::text,
    '2025-01-01 00:00:00',
    'timestamptz(0) also rounds (not truncates)'
);

-- Test 5: Year boundary crossing
SELECT assert_eq(
    ('2024-12-31 23:59:59.6'::timestamp(0))::text,
    '2025-01-01 00:00:00',
    'timestamp(0) rounding can cross year boundary'
);

-- Test 6: Month boundary crossing
SELECT assert_eq(
    ('2024-01-31 23:59:59.5'::timestamp(0))::text,
    '2024-02-01 00:00:00',
    'timestamp(0) rounding can cross month boundary'
);
