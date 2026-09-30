package com.oxilitedb.oxilite.model;

/** The default graph; there is exactly one, {@link #INSTANCE}. */
public final class DefaultGraph implements GraphName {
    public static final DefaultGraph INSTANCE = new DefaultGraph();

    private DefaultGraph() {}

    @Override
    public String getTermType() {
        return "DefaultGraph";
    }

    @Override
    public String toString() {
        return "DEFAULT";
    }
}
