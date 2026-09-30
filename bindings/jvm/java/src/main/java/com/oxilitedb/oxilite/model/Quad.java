package com.oxilitedb.oxilite.model;

import java.util.Objects;

/** A subject/predicate/object statement in a named graph (or the default graph). */
public final class Quad {
    private final Term subject;
    private final NamedNode predicate;
    private final Term object;
    private final GraphName graph;

    public Quad(Term subject, NamedNode predicate, Term object) {
        this(subject, predicate, object, DefaultGraph.INSTANCE);
    }

    public Quad(Term subject, NamedNode predicate, Term object, GraphName graph) {
        this.subject = Objects.requireNonNull(subject, "subject");
        this.predicate = Objects.requireNonNull(predicate, "predicate");
        this.object = Objects.requireNonNull(object, "object");
        this.graph = graph == null ? DefaultGraph.INSTANCE : graph;
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

    public GraphName getGraph() {
        return graph;
    }

    @Override
    public boolean equals(Object o) {
        if (!(o instanceof Quad)) {
            return false;
        }
        Quad q = (Quad) o;
        return subject.equals(q.subject)
                && predicate.equals(q.predicate)
                && object.equals(q.object)
                && graph.equals(q.graph);
    }

    @Override
    public int hashCode() {
        return Objects.hash(subject, predicate, object, graph);
    }

    @Override
    public String toString() {
        String g = graph instanceof DefaultGraph ? "" : " " + graph;
        return subject + " " + predicate + " " + object + g + " .";
    }
}
