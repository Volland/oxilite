package com.oxilitedb.oxilite.model;

import java.util.Objects;

/** A blank node, identified by a label unique within this store. */
public final class BlankNode implements Term, GraphName {
    private final String value;

    public BlankNode(String value) {
        this.value = Objects.requireNonNull(value, "value");
    }

    public String getValue() {
        return value;
    }

    @Override
    public String getTermType() {
        return "BlankNode";
    }

    @Override
    public boolean equals(Object o) {
        return o instanceof BlankNode && value.equals(((BlankNode) o).value);
    }

    @Override
    public int hashCode() {
        return Objects.hash("BlankNode", value);
    }

    @Override
    public String toString() {
        return "_:" + value;
    }
}
