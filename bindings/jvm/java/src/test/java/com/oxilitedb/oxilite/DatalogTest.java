package com.oxilitedb.oxilite;

import static org.junit.jupiter.api.Assertions.assertEquals;

import com.fasterxml.jackson.databind.JsonNode;
import java.util.ArrayList;
import java.util.List;
import org.junit.jupiter.api.Test;

// @lat: [[tests#JVM bindings#Datalog evaluates recursive rules]]
class DatalogTest {
    private static final String PREFIX = "@prefix ex: <http://example.org/> .\n";

    private static Store store() {
        Store s = new Store();
        s.load(
                "@prefix ex: <http://example.org/> .\n"
                        + "ex:ada ex:parent ex:bob . ex:bob ex:parent ex:cy . ex:cy ex:parent ex:dee .",
                "text/turtle");
        return s;
    }

    @Test
    void returnsSolutionsAsTerms() {
        try (Store store = store()) {
            JsonNode r = store.datalog(PREFIX + "p(?x, ?y) :- ex:parent(?x, ?y).\n?- p(?x, ?y).");
            assertEquals("[\"x\",\"y\"]", r.get("columns").toString());
            assertEquals(3, r.get("rows").size());
            assertEquals("NamedNode", r.get("rows").get(0).get(0).get("termType").asText());
        }
    }

    @Test
    void evaluatesLinearRecursion() {
        try (Store store = store()) {
            JsonNode r = store.datalog(
                    PREFIX
                            + "anc(?x, ?y) :- ex:parent(?x, ?y).\n"
                            + "anc(?x, ?z) :- ex:parent(?x, ?y), anc(?y, ?z).\n"
                            + "?- anc(ex:ada, ?who).");
            List<String> who = new ArrayList<>();
            for (JsonNode row : r.get("rows")) {
                who.add(row.get(0).get("value").asText());
            }
            who.sort(String::compareTo);
            assertEquals(
                    List.of(
                            "http://example.org/bob",
                            "http://example.org/cy",
                            "http://example.org/dee"),
                    who);
        }
    }
}
