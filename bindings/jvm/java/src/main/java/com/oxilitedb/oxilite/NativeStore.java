package com.oxilitedb.oxilite;

/**
 * The raw JNI surface: every native method is static and takes the store's {@code handle}
 * explicitly, mirroring {@code bindings/node}'s and {@code bindings/python}'s native class.
 * Every argument/result that carries RDF, options or a result set is a JSON string (see
 * {@code oxilite_core::json} on the Rust side); {@link Store} is the idiomatic wrapper most
 * callers should use instead of this class directly.
 */
final class NativeStore {
    static {
        NativeLoader.ensureLoaded();
    }

    private NativeStore() {}

    static native long nativeCreate(String path, String library, String options);

    static native void nativeClose(long handle);

    static native String nativeQuery(long handle, String sparql, String options);

    static native String nativeExplain(long handle, String sparql);

    static native void nativeUpdate(long handle, String sparql, String baseIri);

    static native String nativeExplainUpdate(long handle, String sparql);

    static native void nativeLoad(
            long handle,
            String data,
            String formatName,
            String baseIri,
            String toGraph,
            boolean bulk);

    static native void nativeAdd(long handle, String quads);

    static native void nativeDelete(long handle, String quads);

    static native boolean nativeHas(long handle, String quad);

    static native String nativeMatch(
            long handle, String subject, String predicate, String object, String graphName);

    static native long nativeSize(long handle);

    static native String nativeDump(long handle, String formatName, String fromGraph);

    static native long nativeMaterialize(long handle, boolean reasonable);

    static native void nativeClearInferences(long handle);

    static native void nativeOptimize(long handle);

    static native void nativeClear(long handle);

    static native void nativeBackup(long handle, String path);

    static native String nativeCypher(long handle, String query, String params, String options);

    static native String nativeExplainCypher(
            long handle, String query, String params, String options);

    static native String nativeDatalog(long handle, String program, String options);

    static native String nativeDatalogMaterialize(long handle, String program, String options);

    static native String nativeExplainDatalog(long handle, String program);

    static native String nativeSynalog(
            long handle, String program, String predicate, String options);

    static native String nativeSynalogSql(
            long handle, String program, String predicate, String options);

    static native String nativeJsonld(long handle, String op, String args, String options);

    static native void nativeRegisterSchemaGraph(
            long handle, String graphJson, String role, String registration);

    static native String nativeSchemaGraphs(long handle);

    static native boolean nativeSetSchemaGraphActive(
            long handle, String graphJson, boolean active);

    static native boolean nativeUnregisterSchemaGraph(long handle, String graphJson);

    static native long nativeDropSchemaGraph(long handle, String graphJson);

    static native String nativeShapeIndex(long handle);

    static native boolean nativeInstallSystemGraphs(long handle);

    static native String nativeSchemaSql(String options);

    static native String nativeVersioning(long handle);

    static native String nativeSetVersioning(long handle, String level, String change);

    static native void nativeSetCommitInfo(long handle, String info);

    static native String nativeHistory(long handle, long limit);

    static native String nativeChanges(long handle, long after, boolean hasUntil, long until);

    static native String nativeDiff(long handle, String from, String to);

    static native void nativePurge(long handle, String pattern, String reason);
}
