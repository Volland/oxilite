package com.oxilitedb.oxilite;

import static org.junit.jupiter.api.Assertions.assertEquals;
import static org.junit.jupiter.api.Assertions.assertFalse;
import static org.junit.jupiter.api.Assertions.assertThrows;
import static org.junit.jupiter.api.Assertions.assertTrue;

import com.fasterxml.jackson.databind.JsonNode;
import com.oxilitedb.oxilite.exceptions.OxiliteSyntaxException;
import com.oxilitedb.oxilite.model.DefaultGraph;
import com.oxilitedb.oxilite.model.Literal;
import com.oxilitedb.oxilite.model.NamedNode;
import com.oxilitedb.oxilite.model.Quad;
import java.util.List;
import org.junit.jupiter.api.Test;

// @lat: [[tests#JVM bindings#Core store round-trips through the native layer]]
class StoreTest {
    @Test
    void insertsAndQueries() {
        try (Store store = new Store()) {
            NamedNode alice = new NamedNode("http://example.org/alice");
            NamedNode name = new NamedNode("http://example.org/name");
            store.add(new Quad(alice, name, new Literal("Alice")));

            assertEquals(1, store.size());
            assertTrue(store.contains(new Quad(alice, name, new Literal("Alice"))));

            JsonNode result = store.query("SELECT ?name WHERE { ?s <http://example.org/name> ?name }");
            assertEquals("solutions", result.get("kind").asText());
            assertEquals("name", result.get("variables").get(0).asText());
            assertEquals(1, result.get("rows").size());
            assertEquals("Alice", result.get("rows").get(0).get(0).get("value").asText());

            List<Quad> matched = store.match(alice, null, null, null);
            assertEquals(1, matched.size());
            assertEquals(DefaultGraph.INSTANCE, matched.get(0).getGraph());

            store.remove(new Quad(alice, name, new Literal("Alice")));
            assertEquals(0, store.size());
        }
    }

    @Test
    void rejectsInvalidSparql() {
        try (Store store = new Store()) {
            assertThrows(OxiliteSyntaxException.class, () -> store.query("SELECT ?x WHERE {"));
        }
    }

    @Test
    void updateAndClear() {
        try (Store store = new Store()) {
            store.update(
                    "INSERT DATA { <http://example.org/s> <http://example.org/p> <http://example.org/o> }");
            assertEquals(1, store.size());
            store.clear();
            assertEquals(0, store.size());
            assertFalse(store.contains(new Quad(
                    new NamedNode("http://example.org/s"),
                    new NamedNode("http://example.org/p"),
                    new NamedNode("http://example.org/o"))));
        }
    }
}
