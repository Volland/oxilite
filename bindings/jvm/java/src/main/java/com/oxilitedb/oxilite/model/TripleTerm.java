package com.oxilitedb.oxilite.model;

import java.util.Objects;

/** An RDF-star quoted triple used as a term. */
public final class TripleTerm implements Term {
    private final Term subject;
    private final NamedNode predicate;
    private final Term object;

    public TripleTerm(Term subject, NamedNode predicate, Term object) {
        this.subject = Objects.requireNonNull(subject, "subject");
        this.predicate = Objects.requireNonNull(predicate, "predicate");
        this.object = Objects.requireNonNull(object, "object");
    }

    public Term getSubject() {
        return subject;
    }

    public NamedNode getPredicate() {
        return predicate;
    }

    public Term getObject() {
        return object;
    }

    @Override
    public String getTermType() {
        return "Quad";
    }

    @Override
    public boolean equals(Object o) {
        if (!(o instanceof TripleTerm)) {
            return false;
        }
        TripleTerm t = (TripleTerm) o;
        return subject.equals(t.subject) && predicate.equals(t.predicate) && object.equals(t.object);
    }

    @Override
    public int hashCode() {
        return Objects.hash(subject, predicate, object);
    }

    @Override
    public String toString() {
        return "<<" + subject + " " + predicate + " " + object + ">>";
    }
}
