package com.oxilitedb.oxilite.exceptions;

/** An RDF document passed to {@code load()} or built from JSON did not parse. */
public class OxiliteParseException extends OxiliteException {
    public OxiliteParseException(String message) {
        super(message);
    }
}
