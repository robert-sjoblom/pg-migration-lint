-- @claim: json must re-parse on every operation; jsonb is faster, smaller, indexable (PGM106)
-- @claim: jsonb supports containment operators (@>, ?, ?|, ?&) but json does not
-- @min_version: 14

-- Test 1: json preserves whitespace and key order; jsonb normalizes
CREATE TABLE test_json(j json, jb jsonb);
INSERT INTO test_json VALUES (
    '{"b": 1, "a":  2, "b": 3}',
    '{"b": 1, "a":  2, "b": 3}'
);

-- json preserves the raw text exactly (including duplicate keys)
SELECT assert_eq(
    (SELECT j::text FROM test_json),
    '{"b": 1, "a":  2, "b": 3}',
    'json preserves original text including whitespace and duplicate keys'
);

-- jsonb normalizes: removes extra whitespace, deduplicates keys (last wins), sorts
SELECT assert_true(
    (SELECT jb::text FROM test_json) != '{"b": 1, "a":  2, "b": 3}',
    'jsonb normalizes the value (different from input text)'
);

-- Test 2: jsonb supports containment operator @>
SELECT assert_true(
    '{"a": 1, "b": 2}'::jsonb @> '{"a": 1}'::jsonb,
    'jsonb supports @> containment operator'
);

-- Test 3: jsonb supports ? (key existence) operator
SELECT assert_true(
    '{"a": 1, "b": 2}'::jsonb ? 'a',
    'jsonb supports ? key existence operator'
);

-- Test 4: json does NOT support containment — this would error
DO $$
DECLARE
    result bool;
BEGIN
    EXECUTE $sql$SELECT '{"a": 1}'::json @> '{"a": 1}'::json$sql$ INTO result;
    PERFORM assert_true(false, 'json does NOT support @> (should have errored)');
EXCEPTION WHEN OTHERS THEN
    PERFORM assert_true(true, 'json does NOT support @> operator (errors as expected)');
END;
$$;

-- Test 5: jsonb supports GIN indexing
CREATE TABLE test_jsonb_idx(data jsonb);
CREATE INDEX idx_jsonb_gin ON test_jsonb_idx USING gin(data);

SELECT assert_eq(
    (SELECT count(*)::bigint FROM pg_indexes
     WHERE tablename = 'test_jsonb_idx' AND indexname = 'idx_jsonb_gin'),
    1::bigint,
    'jsonb supports GIN index creation'
);

-- Cleanup
DROP TABLE test_json;
DROP TABLE test_jsonb_idx;
