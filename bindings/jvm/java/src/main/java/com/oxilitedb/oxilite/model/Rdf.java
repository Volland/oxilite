package com.oxilitedb.oxilite.model;

import com.fasterxml.jackson.databind.JsonNode;
import com.fasterxml.jackson.databind.node.JsonNodeFactory;
import com.fasterxml.jackson.databind.node.ObjectNode;
import com.oxilitedb.oxilite.exceptions.OxiliteException;

/**
 * Converts between the model classes in this package and the RDF/JS-shaped JSON the native
 * layer speaks ({@code {"termType", "value", "language", "direction", "datatype", ...}}), the
 * same wire format used by {@code @oxilite/node} and {@code oxilite} (Python).
 */
public final class Rdf {
    private Rdf() {}

    public static ObjectNode toJson(Term term) {
        JsonNodeFactory f = JsonNodeFactory.instance;
        if (term instanceof NamedNode) {
            ObjectNode n = f.objectNode();
            n.put("termType", "NamedNode");
            n.put("value", ((NamedNode) term).getValue());
            return n;
        }
        if (term instanceof BlankNode) {
            ObjectNode n = f.objectNode();
            n.put("termType", "BlankNode");
            n.put("value", ((BlankNode) term).getValue());
            return n;
        }
        if (term instanceof Literal) {
            Literal l = (Literal) term;
            ObjectNode n = f.objectNode();
            n.put("termType", "Literal");
            n.put("value", l.getValue());
            n.put("language", l.getLanguage());
            if (l.getDirection() != null) {
                n.put("direction", l.getDirection());
            }
            n.set("datatype", toJson(l.getDatatype()));
            return n;
        }
        if (term instanceof TripleTerm) {
            TripleTerm t = (TripleTerm) term;
            ObjectNode n = f.objectNode();
            n.put("termType", "Quad");
            n.put("value", "");
            n.set("subject", toJson(t.getSubject()));
            n.set("predicate", toJson(t.getPredicate()));
            n.set("object", toJson(t.getObject()));
            ObjectNode dg = f.objectNode();
            dg.put("termType", "DefaultGraph");
            dg.put("value", "");
            n.set("graph", dg);
            return n;
        }
        throw new IllegalArgumentException("unknown term type: " + term.getClass());
    }

    public static ObjectNode graphToJson(GraphName graph) {
        JsonNodeFactory f = JsonNodeFactory.instance;
        if (graph instanceof DefaultGraph) {
            ObjectNode n = f.objectNode();
            n.put("termType", "DefaultGraph");
            n.put("value", "");
            return n;
        }
        return toJson((Term) graph);
    }

    public static ObjectNode toJson(Quad quad) {
        JsonNodeFactory f = JsonNodeFactory.instance;
        ObjectNode n = f.objectNode();
        n.put("termType", "Quad");
        n.put("value", "");
        n.set("subject", toJson(quad.getSubject()));
        n.set("predicate", toJson(quad.getPredicate()));
        n.set("object", toJson(quad.getObject()));
        n.set("graph", graphToJson(quad.getGraph()));
        return n;
    }

    public static Term termFromJson(JsonNode v) {
        String type = requireField(v, "termType");
        switch (type) {
            case "NamedNode":
                return new NamedNode(requireField(v, "value"));
            case "BlankNode":
                return new BlankNode(requireField(v, "value"));
            case "Literal": {
                String value = requireField(v, "value");
                String language = v.path("language").asText("");
                String direction = v.hasNonNull("direction") ? v.get("direction").asText() : null;
                NamedNode datatype = v.hasNonNull("datatype")
                        ? (NamedNode) termFromJson(v.get("datatype"))
                        : new NamedNode("http://www.w3.org/2001/XMLSchema#string");
                return new Literal(value, language, direction, datatype);
            }
            case "Quad": {
                Term subject = termFromJson(requireNode(v, "subject"));
                NamedNode predicate = (NamedNode) termFromJson(requireNode(v, "predicate"));
                Term object = termFromJson(requireNode(v, "object"));
                return new TripleTerm(subject, predicate, object);
            }
            default:
                throw new OxiliteException("unsupported termType " + type);
        }
    }

    public static GraphName graphFromJson(JsonNode v) {
        if (v == null || v.isNull() || "DefaultGraph".equals(v.path("termType").asText())) {
            return DefaultGraph.INSTANCE;
        }
        Term t = termFromJson(v);
        if (t instanceof GraphName) {
            return (GraphName) t;
        }
        throw new OxiliteException("a graph name must be an IRI, a blank node, or the default graph");
    }

    public static Quad quadFromJson(JsonNode v) {
        Term subject = termFromJson(requireNode(v, "subject"));
        NamedNode predicate = (NamedNode) termFromJson(requireNode(v, "predicate"));
        Term object = termFromJson(requireNode(v, "object"));
        GraphName graph = v.has("graph") ? graphFromJson(v.get("graph")) : DefaultGraph.INSTANCE;
        return new Quad(subject, predicate, object, graph);
    }

    private static String requireField(JsonNode v, String key) {
        JsonNode f = v.get(key);
        if (f == null || !f.isTextual()) {
            throw new OxiliteException("term JSON without \"" + key + "\": " + v);
        }
        return f.asText();
    }

    private static JsonNode requireNode(JsonNode v, String key) {
        JsonNode f = v.get(key);
        if (f == null) {
            throw new OxiliteException("quad JSON without \"" + key + "\": " + v);
        }
        return f;
    }
}
