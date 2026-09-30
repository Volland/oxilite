package com.oxilitedb.oxilite.exceptions;

/** A SPARQL, Cypher, Datalog or Synalog program did not parse. */
public class OxiliteSyntaxException extends OxiliteException {
    public OxiliteSyntaxException(String message) {
        super(message);
    }
}
