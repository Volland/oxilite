package com.oxilitedb.oxilite;

import static org.junit.jupiter.api.Assertions.assertEquals;

import com.fasterxml.jackson.databind.JsonNode;
import org.junit.jupiter.api.Test;

// @lat: [[tests#JVM bindings#Versioning records commits and time travels]]
class VersioningTest {
    @Test
    void recordsCommitsAndTimeTravels() {
        try (Store store = new Store(null, null, "{\"versioning\": \"log\"}")) {
            store.setCommitInfo("{\"author\": \"ada\", \"message\": \"seed\"}");
            store.update(
                    "INSERT DATA { <http://example.com/t1> <http://example.com/status> \"open\" }");
            store.setCommitInfo(null);
            store.update(
                    "DELETE DATA { <http://example.com/t1> <http://example.com/status> \"open\" } ;"
                            + " INSERT DATA { <http://example.com/t1> <http://example.com/status> \"done\" }");

            JsonNode now = store.query(
                    "SELECT ?s { <http://example.com/t1> <http://example.com/status> ?s }");
            assertEquals("done", now.get("rows").get(0).get(0).get("value").asText());

            JsonNode then = store.query(
                    "SELECT ?s { <http://example.com/t1> <http://example.com/status> ?s }",
                    "{\"asOf\": \"HEAD~1\"}");
            assertEquals("open", then.get("rows").get(0).get(0).get("value").asText());

            JsonNode history = store.history(10);
            String seedAuthor = null;
            for (JsonNode entry : history) {
                if (entry.path("message").asText("").equals("seed")) {
                    seedAuthor = entry.path("author").asText(null);
                }
            }
            assertEquals("ada", seedAuthor);

            assertEquals("log", store.versioning().get("level").asText());
        }
    }
}
