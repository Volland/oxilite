package com.oxilitedb.oxilite.model;

import java.util.Objects;

/** A literal: a lexical value, an optional language tag and base direction, and a datatype. */
public final class Literal implements Term {
    private final String value;
    private final String language;
    private final String direction;
    private final NamedNode datatype;

    private static final NamedNode STRING = new NamedNode("http://www.w3.org/2001/XMLSchema#string");
    private static final NamedNode LANG_STRING =
            new NamedNode("http://www.w3.org/1999/02/22-rdf-syntax-ns#langString");

    public Literal(String value) {
        this(value, "", null, STRING);
    }

    public Literal(String value, String language) {
        this(value, language, null, LANG_STRING);
    }

    public Literal(String value, NamedNode datatype) {
        this(value, "", null, datatype);
    }

    /** {@code direction} is {@code "ltr"}, {@code "rtl"}, or {@code null}. */
    public Literal(String value, String language, String direction, NamedNode datatype) {
        this.value = Objects.requireNonNull(value, "value");
        this.language = language == null ? "" : language;
        this.direction = direction;
        this.datatype = Objects.requireNonNull(datatype, "datatype");
    }

    public String getValue() {
        return value;
    }

    /** The BCP 47 language tag, or {@code ""} when this literal has none. */
    public String getLanguage() {
        return language;
    }

    /** The base direction ({@code "ltr"} / {@code "rtl"}), or {@code null}. */
    public String getDirection() {
        return direction;
    }

    public NamedNode getDatatype() {
        return datatype;
    }

    @Override
    public String getTermType() {
        return "Literal";
    }

    @Override
    public boolean equals(Object o) {
        if (!(o instanceof Literal)) {
            return false;
        }
        Literal l = (Literal) o;
        return value.equals(l.value)
                && language.equals(l.language)
                && Objects.equals(direction, l.direction)
                && datatype.equals(l.datatype);
    }

    @Override
    public int hashCode() {
        return Objects.hash(value, language, direction, datatype);
    }

    @Override
    public String toString() {
        if (!language.isEmpty()) {
            return "\"" + value + "\"@" + language;
        }
        if (datatype.equals(STRING)) {
            return "\"" + value + "\"";
        }
        return "\"" + value + "\"^^" + datatype;
    }
}
