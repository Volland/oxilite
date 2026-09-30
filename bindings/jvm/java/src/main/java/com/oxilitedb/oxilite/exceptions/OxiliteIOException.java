package com.oxilitedb.oxilite.exceptions;

/**
 * An I/O error while reading or writing RDF or the database file. Unchecked (unlike
 * {@link java.io.IOException}) so native method signatures stay {@code throws}-free.
 */
public class OxiliteIOException extends OxiliteException {
    public OxiliteIOException(String message) {
        super(message);
    }
}
