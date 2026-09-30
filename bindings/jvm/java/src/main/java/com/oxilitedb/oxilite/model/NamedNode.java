package com.oxilitedb.oxilite.model;

import java.util.Objects;

/** An IRI. */
public final class NamedNode implements Term, GraphName {
    private final String value;

    public NamedNode(String value) {
        this.value = Objects.requireNonNull(value, "value");
    }

    public String getValue() {
        return value;
    }

    @Override
    public String getTermType() {
        return "NamedNode";
    }

    @Override
    public boolean equals(Object o) {
        return o instanceof NamedNode && value.equals(((NamedNode) o).value);
    }

    @Override
    public int hashCode() {
        return Objects.hash("NamedNode", value);
    }

    @Override
    public String toString() {
        return "<" + value + ">";
    }
}
