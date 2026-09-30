package com.oxilitedb.oxilite;

import static org.junit.jupiter.api.Assertions.assertEquals;
import static org.junit.jupiter.api.Assertions.assertTrue;

import com.fasterxml.jackson.databind.JsonNode;
import org.junit.jupiter.api.Test;

// @lat: [[tests#JVM bindings#JSON-LD documents round-trip through the native layer]]
class JsonLdTest {
    private static final String PERSON =
            "{\"@context\": {\"name\": \"http://schema.org/name\"}, \"@id\": \"urn:uuid:1234\", \"name\": \"Ada\"}";

    @Test
    void storesDocumentsVerbatimAndQueryable() {
        try (Store store = new Store()) {
            JsonNode put = store.jsonld(
                    "put",
                    "{\"documents\": [{\"json\": " + quote(PERSON) + "}]}",
                    null);
            assertEquals("urn:uuid:1234", put.get("keys").get(0).asText());

            JsonNode doc = store.jsonld("get", "{\"key\": \"urn:uuid:1234\"}", null);
            assertEquals(PERSON, doc.get("json").asText());
            assertEquals("urn:uuid:1234", doc.get("graph").get("value").asText());

            JsonNode ask = store.query(
                    "ASK { GRAPH <urn:uuid:1234> { ?s <http://schema.org/name> \"Ada\" } }");
            assertTrue(ask.get("value").asBoolean());

            JsonNode removed = store.jsonld("remove", "{\"key\": \"urn:uuid:1234\"}", null);
            assertTrue(removed.asBoolean());
        }
    }

    private static String quote(String s) {
        return com.fasterxml.jackson.databind.node.JsonNodeFactory.instance.textNode(s).toString();
    }
}
