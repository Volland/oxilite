package com.oxilitedb.oxilite.exceptions;

/** A JSON-LD or Verifiable Credentials error; {@link #getCode()} is the JSON-LD API error code. */
public class JsonLdException extends OxiliteException {
    private final String code;

    public JsonLdException(String message, String code) {
        super(message);
        this.code = code;
    }

    /** The JSON-LD API error code (e.g. {@code "invalid local context"}), or {@code null}. */
    public String getCode() {
        return code;
    }
}
