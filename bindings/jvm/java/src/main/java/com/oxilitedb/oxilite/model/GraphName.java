package com.oxilitedb.oxilite.model;

/** The graph name of a quad: a {@link NamedNode}, a {@link BlankNode}, or {@link DefaultGraph}. */
public interface GraphName {
    String getTermType();
}
