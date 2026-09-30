package com.oxilitedb.oxilite.exceptions;

/** Base, unchecked exception for every error oxilite's native store can raise. */
public class OxiliteException extends RuntimeException {
    public OxiliteException(String message) {
        super(message);
    }

    public OxiliteException(String message, Throwable cause) {
        super(message, cause);
    }
}
