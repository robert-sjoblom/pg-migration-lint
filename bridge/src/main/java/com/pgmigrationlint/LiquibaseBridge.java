package com.pgmigrationlint;

import com.google.gson.Gson;
import com.google.gson.GsonBuilder;
import liquibase.changelog.ChangeLogParameters;
import liquibase.changelog.ChangeSet;
import liquibase.changelog.DatabaseChangeLog;
import liquibase.database.Database;
import liquibase.database.DatabaseFactory;
import liquibase.database.OfflineConnection;
import liquibase.parser.ChangeLogParser;
import liquibase.parser.ChangeLogParserFactory;
import liquibase.resource.DirectoryResourceAccessor;
import liquibase.resource.ResourceAccessor;
import liquibase.sql.Sql;
import liquibase.sqlgenerator.SqlGeneratorFactory;
import liquibase.statement.SqlStatement;

import java.io.File;
import java.io.IOException;
import java.nio.file.Files;
import java.nio.file.Path;
import java.util.ArrayList;
import java.util.HashMap;
import java.util.List;
import java.util.Map;

/**
 * Minimal CLI that embeds Liquibase and produces JSON output mapping changesets
 * to their SQL statements with line numbers.
 *
 * Usage: java -jar liquibase-bridge.jar --changelog <path>
 *
 * Output: JSON array to stdout matching the format expected by pg-migration-lint's
 * BridgeChangeset struct.
 */
public class LiquibaseBridge {

    public static void main(String[] args) {
        String changelogPath = null;

        for (int i = 0; i < args.length; i++) {
            if ("--changelog".equals(args[i]) && i + 1 < args.length) {
                changelogPath = args[i + 1];
                i++;
            }
        }

        if (changelogPath == null) {
            System.err.println("Usage: java -jar liquibase-bridge.jar --changelog <path>");
            System.exit(2);
        }

        try {
            List<ChangesetEntry> entries = processChangelog(changelogPath);
            Gson gson = new GsonBuilder().setPrettyPrinting().create();
            System.out.println(gson.toJson(entries));
        } catch (Exception e) {
            System.err.println("Error processing changelog: " + e.getMessage());
            e.printStackTrace(System.err);
            System.exit(1);
        }
    }

    static List<ChangesetEntry> processChangelog(String changelogPath) throws Exception {
        File changelogFile = new File(changelogPath).getAbsoluteFile();
        if (!changelogFile.exists()) {
            throw new IllegalArgumentException("Changelog file not found: " + changelogFile);
        }

        // Use the directory containing the changelog as the resource root,
        // so that relative <include> paths resolve correctly.
        Path resourceRoot = changelogFile.getParentFile().toPath();
        String relativeChangelog = resourceRoot.relativize(changelogFile.toPath()).toString();

        ResourceAccessor resourceAccessor = new DirectoryResourceAccessor(resourceRoot);

        // Use an offline PostgreSQL connection so Liquibase can generate SQL
        // without requiring an actual database.
        OfflineConnection connection = new OfflineConnection(
                "offline:postgresql?outputLiquibaseSql=none",
                resourceAccessor
        );
        Database database = DatabaseFactory.getInstance().findCorrectDatabaseImplementation(connection);

        ChangeLogParser parser = ChangeLogParserFactory.getInstance()
                .getParser(relativeChangelog, resourceAccessor);
        DatabaseChangeLog changeLog = parser.parse(relativeChangelog,
                new ChangeLogParameters(database), resourceAccessor);

        List<ChangesetEntry> entries = new ArrayList<>();
        int skippedCount = 0;

        for (ChangeSet changeSet : changeLog.getChangeSets()) {
            try {
                StringBuilder sqlBuilder = new StringBuilder();

                // Generate SQL for each change within the changeset.
                for (var change : changeSet.getChanges()) {
                    SqlStatement[] statements = change.generateStatements(database);
                    for (SqlStatement statement : statements) {
                        Sql[] sqls = SqlGeneratorFactory.getInstance()
                                .generateSql(statement, database);
                        for (Sql sql : sqls) {
                            if (sqlBuilder.length() > 0) {
                                sqlBuilder.append("\n");
                            }
                            sqlBuilder.append(sql.toSql()).append(";");
                        }
                    }
                }

                // Also capture any raw SQL blocks within the changeset.
                // Liquibase <sql> tags store their content differently.
                String generatedSql = sqlBuilder.toString();
                if (generatedSql.isEmpty()) {
                    // Skip changesets that produce no SQL (e.g., preconditions-only).
                    continue;
                }

                ChangesetEntry entry = new ChangesetEntry();
                entry.changeset_id = changeSet.getId();
                entry.author = changeSet.getAuthor() != null ? changeSet.getAuthor() : "";
                entry.sql = generatedSql;

                // Resolve the XML file path relative to the original changelog location,
                // preserving the path the user provided.
                String filePath = changeSet.getFilePath();
                if (filePath != null) {
                    entry.xml_file = filePath;
                } else {
                    entry.xml_file = changelogPath;
                }

                // Liquibase does not expose the XML line number directly in all versions,
                // so we default to 1 if unavailable. The Rust side handles this gracefully.
                entry.xml_line = 1;

                entry.run_in_transaction = changeSet.isRunInTransaction();

                entries.add(entry);
            } catch (Exception e) {
                skippedCount++;
                System.err.println("WARNING: Skipped changeset '"
                    + changeSet.getId() + "' (" + changeSet.getFilePath()
                    + "): " + e.getMessage());
            }
        }

        if (skippedCount > 0) {
            System.err.println("WARNING: " + skippedCount
                + " changeset(s) skipped due to SQL generation errors");
        }

        resolveXmlLineNumbers(entries, resourceRoot);

        return entries;
    }

    /**
     * Post-process entries to resolve XML line numbers by scanning the source files
     * for changeset ID attributes. Falls back to line 1 if the file cannot be read
     * or the changeset ID is not found.
     */
    static void resolveXmlLineNumbers(List<ChangesetEntry> entries, Path resourceRoot) {
        // Group entries by xml_file to avoid re-reading the same file
        Map<String, List<ChangesetEntry>> byFile = new HashMap<>();
        for (ChangesetEntry entry : entries) {
            byFile.computeIfAbsent(entry.xml_file, k -> new ArrayList<>()).add(entry);
        }

        for (Map.Entry<String, List<ChangesetEntry>> group : byFile.entrySet()) {
            Path filePath = resourceRoot.resolve(group.getKey());
            List<String> lines;
            try {
                lines = Files.readAllLines(filePath);
            } catch (IOException e) {
                // Can't read the file; leave all entries at default (1)
                continue;
            }

            for (ChangesetEntry entry : group.getValue()) {
                String needle = "id=\"" + entry.changeset_id + "\"";
                for (int i = 0; i < lines.size(); i++) {
                    if (lines.get(i).contains(needle)) {
                        entry.xml_line = i + 1; // 1-based
                        break;
                    }
                }
            }
        }
    }

    /**
     * JSON output structure matching the BridgeChangeset Rust struct.
     */
    @SuppressWarnings("unused")
    static class ChangesetEntry {
        String changeset_id;
        String author;
        String sql;
        String xml_file;
        int xml_line;
        boolean run_in_transaction;
    }
}
