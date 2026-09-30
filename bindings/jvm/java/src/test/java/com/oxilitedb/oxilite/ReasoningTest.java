package com.oxilitedb.oxilite;

import static org.junit.jupiter.api.Assertions.assertEquals;
import static org.junit.jupiter.api.Assertions.assertTrue;

import com.fasterxml.jackson.databind.JsonNode;
import org.junit.jupiter.api.Test;

// @lat: [[tests#JVM bindings#Reasons per query and materializes OWL 2 RL]]
class ReasoningTest {
    private static final String QUERY = "SELECT ?x WHERE { ?x a <http://example.com/Animal> }";

    private static Store loaded() {
        Store store = new Store();
        store.load(
                "@prefix ex: <http://example.com/> . @prefix rdfs: <http://www.w3.org/2000/01/rdf-schema#> ."
                        + " @prefix owl: <http://www.w3.org/2002/07/owl#> ."
                        + " ex:Dog rdfs:subClassOf ex:Animal . ex:rex a ex:Dog . ex:rex owl:sameAs ex:rexy ."
                        + " ex:rex ex:name \"Rex\" .",
                "text/turtle");
        return store;
    }

    @Test
    void reasonsPerQueryAndMaterializes() {
        try (Store store = loaded()) {
            assertEquals(0, store.query(QUERY).get("rows").size());
            assertEquals(
                    1, store.query(QUERY, "{\"reasoning\": \"rdfs\"}").get("rows").size());

            for (boolean reasonable : new boolean[] {false, true}) {
                assertTrue(store.materialize(reasonable) > 0);
                JsonNode name = store.query(
                        "SELECT ?n WHERE { <http://example.com/rexy> <http://example.com/name> ?n }",
                        "{\"include_inferred\": true}");
                assertEquals("Rex", name.get("rows").get(0).get(0).get("value").asText());
            }

            store.clearInferences();
            assertEquals(
                    0,
                    store.query(QUERY, "{\"include_inferred\": true}").get("rows").size());
        }
    }
}
