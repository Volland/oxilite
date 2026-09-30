package com.oxilitedb.oxilite.model;

/**
 * An RDF term: a {@link NamedNode}, {@link BlankNode}, {@link Literal}, or (RDF-star) a
 * {@link TripleTerm}. Mirrors the RDF/JS {@code Term} union oxilite's other bindings use.
 */
public interface Term {
    /** {@code "NamedNode"}, {@code "BlankNode"}, {@code "Literal"} or {@code "Quad"}. */
    String getTermType();
}
