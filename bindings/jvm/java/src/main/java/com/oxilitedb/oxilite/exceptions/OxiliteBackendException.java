package com.oxilitedb.oxilite.exceptions;

/** The SQL backend failed, the database is corrupted, or a term hash collided. */
public class OxiliteBackendException extends OxiliteException {
    public OxiliteBackendException(String message) {
        super(message);
    }
}
