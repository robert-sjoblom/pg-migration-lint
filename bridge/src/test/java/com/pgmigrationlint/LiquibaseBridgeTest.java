package com.pgmigrationlint;

import org.junit.jupiter.api.Test;

import java.io.ByteArrayOutputStream;
import java.io.PrintStream;
import java.net.URISyntaxException;
import java.nio.file.Paths;
import java.util.List;
import java.util.Map;
import java.util.stream.Collectors;

import static org.junit.jupiter.api.Assertions.*;

class LiquibaseBridgeTest {

    private String fixturePath(String name) throws URISyntaxException {
        var url = getClass().getClassLoader().getResource("fixtures/" + name);
        assertNotNull(url, "Fixture not found: " + name);
        return Paths.get(url.toURI()).toString();
    }

    @Test
    void basicCreateTableProducesCorrectEntries() throws Exception {
        List<LiquibaseBridge.ChangesetEntry> entries =
            LiquibaseBridge.processChangelog(fixturePath("basic-create-table.xml"));

        assertEquals(1, entries.size());

        LiquibaseBridge.ChangesetEntry entry = entries.get(0);
        assertEquals("1", entry.changeset_id);
        assertEquals("testauthor", entry.author);
        assertTrue(entry.run_in_transaction);
        assertTrue(entry.sql.toUpperCase().contains("CREATE TABLE"),
            "Expected CREATE TABLE in SQL: " + entry.sql);
        assertTrue(entry.sql.toLowerCase().contains("widgets"),
            "Expected 'widgets' in SQL: " + entry.sql);
    }

    @Test
    void unsupportedChangesetIsSkippedOthersSucceed() throws Exception {
        PrintStream originalErr = System.err;
        ByteArrayOutputStream errCapture = new ByteArrayOutputStream();
        System.setErr(new PrintStream(errCapture));

        List<LiquibaseBridge.ChangesetEntry> entries;
        try {
            entries = LiquibaseBridge.processChangelog(fixturePath("skip-unsupported.xml"));
        } finally {
            System.setErr(originalErr);
        }

        List<String> ids = entries.stream().map(e -> e.changeset_id).toList();
        assertTrue(ids.contains("skip-1"), "skip-1 should be present");
        assertTrue(ids.contains("skip-3"), "skip-3 should be present");
        assertFalse(ids.contains("skip-2"), "loadData changeset should be skipped");

        String stderr = errCapture.toString();
        assertTrue(stderr.contains("skip-2"), "Warning should mention skipped changeset id");
    }

    @Test
    void changesetWithNoSqlProducesEmptyResult() throws Exception {
        List<LiquibaseBridge.ChangesetEntry> entries =
            LiquibaseBridge.processChangelog(fixturePath("empty-no-sql.xml"));

        assertTrue(entries.isEmpty(),
            "Expected empty result for no-SQL changesets, got " + entries.size());
    }

    @Test
    void metadataFieldsAreCorrectlyPopulated() throws Exception {
        List<LiquibaseBridge.ChangesetEntry> entries =
            LiquibaseBridge.processChangelog(fixturePath("basic-create-table.xml"));

        assertEquals(1, entries.size());
        LiquibaseBridge.ChangesetEntry entry = entries.get(0);

        assertEquals("1", entry.changeset_id);
        assertEquals("testauthor", entry.author);
        assertTrue(entry.xml_file.contains("basic-create-table.xml"),
            "xml_file should reference the changelog file, got: " + entry.xml_file);
        assertEquals(7, entry.xml_line, "Should resolve to the <changeSet> line in the XML");
        assertTrue(entry.run_in_transaction);
    }

    @Test
    void multiChangeChangesetProducesCombinedSql() throws Exception {
        List<LiquibaseBridge.ChangesetEntry> entries =
            LiquibaseBridge.processChangelog(fixturePath("multi-change-changeset.xml"));

        assertEquals(1, entries.size());

        String sql = entries.get(0).sql.toUpperCase();
        assertTrue(sql.contains("CREATE TABLE"), "Should contain CREATE TABLE");
        assertTrue(sql.contains("CREATE INDEX"), "Should contain CREATE INDEX");
        assertTrue(sql.contains("IDX_ORDERS_CUSTOMER"), "Should reference the index name");
    }

    @Test
    void rawSqlTagsProduceCorrectOutput() throws Exception {
        List<LiquibaseBridge.ChangesetEntry> entries =
            LiquibaseBridge.processChangelog(fixturePath("raw-sql.xml"));

        assertEquals(2, entries.size());

        assertEquals("raw-1", entries.get(0).changeset_id);
        assertTrue(entries.get(0).sql.contains("CONCURRENTLY"),
            "First raw SQL should contain CONCURRENTLY");

        assertEquals("raw-2", entries.get(1).changeset_id);
        assertTrue(entries.get(1).sql.contains("CHECK"),
            "Second raw SQL should contain CHECK");
    }

    @Test
    void missingChangelogThrowsException() {
        assertThrows(IllegalArgumentException.class, () ->
            LiquibaseBridge.processChangelog("/nonexistent/path/changelog.xml"));
    }

    @Test
    void runInTransactionCapturedCorrectly() throws Exception {
        List<LiquibaseBridge.ChangesetEntry> entries =
            LiquibaseBridge.processChangelog(fixturePath("run-in-transaction.xml"));

        assertEquals(2, entries.size());

        LiquibaseBridge.ChangesetEntry defaultTxn = entries.stream()
            .filter(e -> "txn-default".equals(e.changeset_id))
            .findFirst().orElseThrow();
        assertTrue(defaultTxn.run_in_transaction, "Default should be true");

        LiquibaseBridge.ChangesetEntry falseTxn = entries.stream()
            .filter(e -> "txn-false".equals(e.changeset_id))
            .findFirst().orElseThrow();
        assertFalse(falseTxn.run_in_transaction, "Explicit false should be false");
    }

    @Test
    void includeDirectiveResolvesChildFile() throws Exception {
        List<LiquibaseBridge.ChangesetEntry> entries =
            LiquibaseBridge.processChangelog(fixturePath("include-directive.xml"));

        assertEquals(2, entries.size());

        LiquibaseBridge.ChangesetEntry master = entries.stream()
            .filter(e -> "master-1".equals(e.changeset_id))
            .findFirst().orElseThrow();
        assertTrue(master.xml_file.contains("include-directive.xml"),
            "Master changeset should reference master file, got: " + master.xml_file);

        LiquibaseBridge.ChangesetEntry child = entries.stream()
            .filter(e -> "child-1".equals(e.changeset_id))
            .findFirst().orElseThrow();
        assertTrue(child.xml_file.contains("include-child.xml"),
            "Child changeset should reference child file, got: " + child.xml_file);
    }

    @Test
    void xmlLineNumbersResolvedForMultiChangesetFile() throws Exception {
        List<LiquibaseBridge.ChangesetEntry> entries =
            LiquibaseBridge.processChangelog(fixturePath("mixed-ddl.xml"));

        Map<String, Integer> lineById = entries.stream()
            .collect(Collectors.toMap(e -> e.changeset_id, e -> e.xml_line));

        // Each changeset should point to its own <changeSet> line, not default 1
        assertEquals(7, lineById.get("mix-1"), "mix-1 starts at line 7");
        assertEquals(18, lineById.get("mix-2"), "mix-2 starts at line 18");
        assertEquals(32, lineById.get("mix-3"), "mix-3 starts at line 32");
        assertEquals(38, lineById.get("mix-4"), "mix-4 starts at line 38");
        assertEquals(49, lineById.get("mix-5"), "mix-5 starts at line 49");
    }

    @Test
    void xmlLineNumbersResolvedForIncludedChildFile() throws Exception {
        List<LiquibaseBridge.ChangesetEntry> entries =
            LiquibaseBridge.processChangelog(fixturePath("include-directive.xml"));

        LiquibaseBridge.ChangesetEntry master = entries.stream()
            .filter(e -> "master-1".equals(e.changeset_id))
            .findFirst().orElseThrow();
        assertEquals(7, master.xml_line,
            "Master changeset should resolve to its own file's line");

        LiquibaseBridge.ChangesetEntry child = entries.stream()
            .filter(e -> "child-1".equals(e.changeset_id))
            .findFirst().orElseThrow();
        assertEquals(7, child.xml_line,
            "Child changeset should resolve to the child file's line");
    }

    @Test
    void xmlLineNumbersResolvedForSkippedChangesets() throws Exception {
        PrintStream originalErr = System.err;
        System.setErr(new PrintStream(new ByteArrayOutputStream()));
        List<LiquibaseBridge.ChangesetEntry> entries;
        try {
            entries = LiquibaseBridge.processChangelog(fixturePath("skip-unsupported.xml"));
        } finally {
            System.setErr(originalErr);
        }

        Map<String, Integer> lineById = entries.stream()
            .collect(Collectors.toMap(e -> e.changeset_id, e -> e.xml_line));

        // skip-2 is skipped, but skip-1 and skip-3 should have correct lines
        assertEquals(7, lineById.get("skip-1"), "skip-1 starts at line 7");
        assertEquals(22, lineById.get("skip-3"), "skip-3 starts at line 22");
    }
}
