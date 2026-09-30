package com.oxilitedb.oxilite;

import static org.junit.jupiter.api.Assertions.assertEquals;
import static org.junit.jupiter.api.Assertions.assertTrue;
import static org.junit.jupiter.api.Assertions.assertThrows;

import com.fasterxml.jackson.databind.JsonNode;
import com.oxilitedb.oxilite.exceptions.OxiliteException;
import org.junit.jupiter.api.Test;

// @lat: [[tests#JVM bindings#Cypher writes and reads through the native layer]]
class CypherTest {
    private static final String BASE = "{\"base\": \"http://example.com/\"}";

    @Test
    void writesWithCypherAndReadsWithSparql() {
        try (Store store = new Store()) {
            JsonNode created = store.cypher(
                    "CREATE (a:Person {name: $name}) RETURN a", "{\"name\": \"Ada\"}", BASE);
            assertEquals(1, created.get("stats").get("nodesCreated").asInt());

            JsonNode ask = store.query(
                    "ASK { ?p a <http://example.com/Person> ; <http://example.com/name> \"Ada\" }");
            assertTrue(ask.get("value").asBoolean());

            JsonNode read = store.cypher("MATCH (p:Person) RETURN p.name AS name", null, BASE);
            assertEquals("Ada", read.get("rows").get(0).get(0).asText());

            assertTrue(store.explainCypher("MATCH (p:Person) RETURN p", null, BASE).contains("SELECT"));
        }
    }

    @Test
    void reportsCypherErrors() {
        try (Store store = new Store()) {
            assertThrows(OxiliteException.class, () -> store.cypher("MATCH (n) RETURN m", null, null));
        }
    }
}
