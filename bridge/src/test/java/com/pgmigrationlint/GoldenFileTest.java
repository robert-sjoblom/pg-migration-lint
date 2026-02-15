package com.pgmigrationlint;

import com.google.gson.Gson;
import com.google.gson.GsonBuilder;
import org.json.JSONException;
import org.junit.jupiter.api.Test;
import org.junit.jupiter.params.ParameterizedTest;
import org.junit.jupiter.params.provider.ValueSource;
import org.skyscreamer.jsonassert.JSONAssert;
import org.skyscreamer.jsonassert.JSONCompareMode;

import java.io.IOException;
import java.io.InputStream;
import java.net.URISyntaxException;
import java.nio.charset.StandardCharsets;
import java.nio.file.Path;
import java.nio.file.Paths;
import java.util.List;

import static org.junit.jupiter.api.Assertions.*;

/**
 * Golden-file tests that run the bridge against fixture changelogs
 * and compare the JSON output to committed snapshot files.
 *
 * To regenerate golden files after intentional changes, run the bridge
 * against each fixture XML and capture stdout as the .expected.json file.
 */
class GoldenFileTest {

    private static final Gson GSON = new GsonBuilder().setPrettyPrinting().create();

    @ParameterizedTest(name = "golden file: {0}")
    @ValueSource(strings = {
        "basic-create-table",
        "multi-change-changeset",
        "raw-sql",
        "skip-unsupported",
        "run-in-transaction",
        "mixed-ddl",
        "include-directive"
    })
    void goldenFileMatchesExpectedOutput(String fixtureName) throws Exception {
        String xmlPath = fixtureFilePath(fixtureName + ".xml");
        String expectedJson = loadResource("fixtures/" + fixtureName + ".expected.json");

        List<LiquibaseBridge.ChangesetEntry> entries =
            LiquibaseBridge.processChangelog(xmlPath);
        String actualJson = GSON.toJson(entries);

        try {
            JSONAssert.assertEquals(expectedJson, actualJson, JSONCompareMode.STRICT);
        } catch (JSONException e) {
            fail("JSON comparison failed for " + fixtureName + ": " + e.getMessage()
                + "\n\nActual output:\n" + actualJson);
        }
    }

    private String fixtureFilePath(String name) throws URISyntaxException {
        var url = getClass().getClassLoader().getResource("fixtures/" + name);
        assertNotNull(url, "Missing fixture: " + name);
        return Paths.get(url.toURI()).toString();
    }

    private String loadResource(String path) throws IOException {
        try (InputStream is = getClass().getClassLoader().getResourceAsStream(path)) {
            assertNotNull(is, "Missing resource: " + path);
            return new String(is.readAllBytes(), StandardCharsets.UTF_8);
        }
    }

    @Test
    void fullChangelogGoldenFile() throws Exception {
        // Maven CWD is bridge/, so parent is the project root
        Path projectRoot = Paths.get("").toAbsolutePath().getParent();
        Path masterXml = projectRoot.resolve("tests/fixtures/repos/liquibase-xml/changelog/master.xml");

        assertTrue(masterXml.toFile().exists(),
            "Master XML fixture not found at: " + masterXml);

        List<LiquibaseBridge.ChangesetEntry> entries =
            LiquibaseBridge.processChangelog(masterXml.toString());
        String actualJson = GSON.toJson(entries);

        String expectedJson = loadResource("fixtures/full-changelog.expected.json");

        try {
            JSONAssert.assertEquals(expectedJson, actualJson, JSONCompareMode.STRICT);
        } catch (JSONException e) {
            fail("JSON comparison failed for full-changelog: " + e.getMessage()
                + "\n\nActual output:\n" + actualJson);
        }
    }
}
