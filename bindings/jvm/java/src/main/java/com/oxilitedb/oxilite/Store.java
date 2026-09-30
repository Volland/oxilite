package com.oxilitedb.oxilite;

import com.fasterxml.jackson.databind.JsonNode;
import com.fasterxml.jackson.databind.ObjectMapper;
import com.oxilitedb.oxilite.exceptions.OxiliteException;
import com.oxilitedb.oxilite.model.GraphName;
import com.oxilitedb.oxilite.model.NamedNode;
import com.oxilitedb.oxilite.model.Quad;
import com.oxilitedb.oxilite.model.Rdf;
import com.oxilitedb.oxilite.model.Term;
import java.util.ArrayList;
import java.util.Collections;
import java.util.List;
import java.util.concurrent.atomic.AtomicBoolean;
import org.jetbrains.annotations.NotNull;
import org.jetbrains.annotations.Nullable;

/**
 * A SPARQL store on SQLite: bundled, or an externally loaded {@code libsqlite3} shared library.
 * Every JSON-shaped argument/result method takes/returns a Jackson {@link JsonNode}; RDF terms
 * and quads use the typed {@link com.oxilitedb.oxilite.model} classes instead of raw JSON.
 *
 * <p>Usable directly from Java, Kotlin, Scala and Clojure via ordinary Java interop. Not
 * thread-safe for concurrent writes to the same handle beyond what SQLite itself serializes;
 * {@link #close()} once done, ideally via try-with-resources.
 */
public final class Store implements AutoCloseable {
    private static final ObjectMapper MAPPER = new ObjectMapper();

    private final long handle;
    private final AtomicBoolean closed = new AtomicBoolean(false);

    /** An in-memory store. */
    public Store() {
        this(null, null, null);
    }

    /** A store backed by the SQLite database file at {@code path} (created if absent). */
    public Store(@NotNull String path) {
        this(path, null, null);
    }

    /**
     * @param path a database file, or {@code null} for an in-memory store
     * @param library a {@code libsqlite3} shared library to {@code dlopen} instead of the
     *     bundled SQLite, or {@code null}
     * @param optionsJson store options as JSON (e.g. {@code {"graphIndex": true}}), or {@code null}
     */
    public Store(@Nullable String path, @Nullable String library, @Nullable String optionsJson) {
        this.handle = NativeStore.nativeCreate(path, library, optionsJson);
        if (this.handle == 0) {
            throw new OxiliteException("failed to open the oxilite store");
        }
    }

    // --------------------------------------------------------------------------- SPARQL

    /** A SPARQL query; the result shape depends on the query form (see the `kind` field). */
    @NotNull
    public JsonNode query(@NotNull String sparql) {
        return query(sparql, null);
    }

    @NotNull
    public JsonNode query(@NotNull String sparql, @Nullable String optionsJson) {
        return parse(NativeStore.nativeQuery(handle, sparql, optionsJson));
    }

    /** The SQL a query compiles to, with the planner's notes. */
    @NotNull
    public String explain(@NotNull String sparql) {
        return NativeStore.nativeExplain(handle, sparql);
    }

    /** A SPARQL update, applied atomically. */
    public void update(@NotNull String sparql) {
        update(sparql, null);
    }

    public void update(@NotNull String sparql, @Nullable String baseIri) {
        NativeStore.nativeUpdate(handle, sparql, baseIri);
    }

    /** How an update runs: the SQL of each operation, or why it needs the fallback. */
    @NotNull
    public String explainUpdate(@NotNull String sparql) {
        return NativeStore.nativeExplainUpdate(handle, sparql);
    }

    // ------------------------------------------------------------------------------ RDF I/O

    /** Loads RDF from {@code data}, atomically. */
    public void load(@NotNull String data, @NotNull String formatName) {
        load(data, formatName, null, null, false);
    }

    /**
     * Loads RDF from {@code data}; {@code bulk} loads in chunks followed by a statistics
     * refresh, useful for a large one-shot import.
     */
    public void load(
            @NotNull String data,
            @NotNull String formatName,
            @Nullable String baseIri,
            @Nullable GraphName toGraph,
            boolean bulk) {
        String toGraphJson = toGraph == null ? null : Rdf.graphToJson(toGraph).toString();
        NativeStore.nativeLoad(handle, data, formatName, baseIri, toGraphJson, bulk);
    }

    /** Serializes the whole dataset. */
    @NotNull
    public String dump(@NotNull String formatName) {
        return dump(formatName, null);
    }

    /** Serializes one graph, when {@code fromGraph} is given; otherwise the whole dataset. */
    @NotNull
    public String dump(@NotNull String formatName, @Nullable GraphName fromGraph) {
        String graphJson = fromGraph == null ? null : Rdf.graphToJson(fromGraph).toString();
        return NativeStore.nativeDump(handle, formatName, graphJson);
    }

    /** Writes a consistent copy of the database to {@code path} ({@code VACUUM INTO}). */
    public void backup(@NotNull String path) {
        NativeStore.nativeBackup(handle, path);
    }

    // --------------------------------------------------------------------------- quad CRUD

    public void add(@NotNull Quad quad) {
        add(Collections.singletonList(quad));
    }

    /** Inserts quads atomically. */
    public void add(@NotNull List<Quad> quads) {
        NativeStore.nativeAdd(handle, quadsToJson(quads).toString());
    }

    public void remove(@NotNull Quad quad) {
        remove(Collections.singletonList(quad));
    }

    public void remove(@NotNull List<Quad> quads) {
        NativeStore.nativeDelete(handle, quadsToJson(quads).toString());
    }

    public boolean contains(@NotNull Quad quad) {
        return NativeStore.nativeHas(handle, Rdf.toJson(quad).toString());
    }

    /** Quads matching a pattern; {@code null} components match anything. */
    @NotNull
    public List<Quad> match(
            @Nullable Term subject,
            @Nullable NamedNode predicate,
            @Nullable Term object,
            @Nullable GraphName graph) {
        String s = subject == null ? null : Rdf.toJson(subject).toString();
        String p = predicate == null ? null : Rdf.toJson(predicate).toString();
        String o = object == null ? null : Rdf.toJson(object).toString();
        String g = graph == null ? null : Rdf.graphToJson(graph).toString();
        JsonNode result = parse(NativeStore.nativeMatch(handle, s, p, o, g));
        List<Quad> quads = new ArrayList<>();
        for (JsonNode q : result.path("quads")) {
            quads.add(Rdf.quadFromJson(q));
        }
        return quads;
    }

    public long size() {
        return NativeStore.nativeSize(handle);
    }

    // ---------------------------------------------------------------------------- reasoning

    /** Computes the OWL 2 RL closure into the inference table (SQL rules); returns the count. */
    public long materialize() {
        return materialize(false);
    }

    /** Like {@link #materialize()}, but in memory (the `reasonable` engine) when {@code true}. */
    public long materialize(boolean reasonable) {
        return NativeStore.nativeMaterialize(handle, reasonable);
    }

    /** Removes every materialized inference. */
    public void clearInferences() {
        NativeStore.nativeClearInferences(handle);
    }

    // ------------------------------------------------------------------------------- cypher

    @NotNull
    public JsonNode cypher(@NotNull String query) {
        return cypher(query, null, null);
    }

    @NotNull
    public JsonNode cypher(@NotNull String query, @Nullable String paramsJson, @Nullable String optionsJson) {
        return parse(NativeStore.nativeCypher(handle, query, paramsJson, optionsJson));
    }

    @NotNull
    public String explainCypher(@NotNull String query, @Nullable String paramsJson, @Nullable String optionsJson) {
        return NativeStore.nativeExplainCypher(handle, query, paramsJson, optionsJson);
    }

    // ------------------------------------------------------------------------------ datalog

    @NotNull
    public JsonNode datalog(@NotNull String program) {
        return datalog(program, null);
    }

    @NotNull
    public JsonNode datalog(@NotNull String program, @Nullable String optionsJson) {
        return parse(NativeStore.nativeDatalog(handle, program, optionsJson));
    }

    /** Stores what a Datalog program derives as inferences, beside the OWL ones. */
    @NotNull
    public JsonNode datalogMaterialize(@NotNull String program, @Nullable String optionsJson) {
        return parse(NativeStore.nativeDatalogMaterialize(handle, program, optionsJson));
    }

    @NotNull
    public String explainDatalog(@NotNull String program) {
        return NativeStore.nativeExplainDatalog(handle, program);
    }

    // ------------------------------------------------------------------------------ synalog

    @NotNull
    public JsonNode synalog(@NotNull String program, @NotNull String predicate) {
        return synalog(program, predicate, null);
    }

    @NotNull
    public JsonNode synalog(@NotNull String program, @NotNull String predicate, @Nullable String optionsJson) {
        return parse(NativeStore.nativeSynalog(handle, program, predicate, optionsJson));
    }

    /** The SQL a Synalog predicate compiles to on this store. */
    @NotNull
    public String synalogSql(@NotNull String program, @NotNull String predicate, @Nullable String optionsJson) {
        return NativeStore.nativeSynalogSql(handle, program, predicate, optionsJson);
    }

    // ------------------------------------------------------------------------------- JSON-LD

    /**
     * A JSON-LD document or (with {@code "credentials": true} in {@code optionsJson}) a
     * Verifiable Credentials operation ({@code put}, {@code get}, {@code remove}, {@code list},
     * {@code find}, {@code graphs}, {@code documentForGraph}, {@code putContext},
     * {@code removeContext}, {@code contexts}, {@code rebuild}, {@code check},
     * {@code putCredential}, {@code putPresentation}); {@code argsJson} carries the
     * operation's arguments.
     */
    @NotNull
    public JsonNode jsonld(@NotNull String op, @NotNull String argsJson, @Nullable String optionsJson) {
        return parse(NativeStore.nativeJsonld(handle, op, argsJson, optionsJson));
    }

    // --------------------------------------------------------------------------- schema registry

    /**
     * Declares {@code graph} to hold an ontology, SHACL shapes or a ShEx schema; {@code role} is
     * {@code "ontology"}, {@code "shacl"} or {@code "shex"}; {@code registrationJson} is
     * {@code {iri, version, sha256, imports, appliesTo, active}} (all optional), or {@code null}.
     */
    public void registerSchemaGraph(
            @NotNull GraphName graph, @NotNull String role, @Nullable String registrationJson) {
        NativeStore.nativeRegisterSchemaGraph(
                handle, Rdf.graphToJson(graph).toString(), role, registrationJson);
    }

    /** The registry: {@code [{graph, role, iri, version, sha256, imports, appliesTo, active, loadedAt}]}. */
    @NotNull
    public JsonNode schemaGraphs() {
        return parse(NativeStore.nativeSchemaGraphs(handle));
    }

    /** Activates or deactivates a registration; returns whether one was found. */
    public boolean setSchemaGraphActive(@NotNull GraphName graph, boolean active) {
        return NativeStore.nativeSetSchemaGraphActive(handle, Rdf.graphToJson(graph).toString(), active);
    }

    /** Removes a registration, keeping the triples; returns whether one was found. */
    public boolean unregisterSchemaGraph(@NotNull GraphName graph) {
        return NativeStore.nativeUnregisterSchemaGraph(handle, Rdf.graphToJson(graph).toString());
    }

    /** Removes a registration and every quad of its graph; returns the number of quads removed. */
    public long dropSchemaGraph(@NotNull GraphName graph) {
        return NativeStore.nativeDropSchemaGraph(handle, Rdf.graphToJson(graph).toString());
    }

    /** The compiled SHACL property shapes: {@code [{target, path, datatype, minCount, ...}]}. */
    @NotNull
    public JsonNode shapeIndex() {
        return parse(NativeStore.nativeShapeIndex(handle));
    }

    /** Installs or refreshes the system graphs; returns {@code false} when already current. */
    public boolean installSystemGraphs() {
        return NativeStore.nativeInstallSystemGraphs(handle);
    }

    /** The schema as a SQL script, for store options as JSON (or {@code null} for defaults). */
    @NotNull
    public static String schemaSql(@Nullable String optionsJson) {
        return NativeStore.nativeSchemaSql(optionsJson);
    }

    // ---------------------------------------------------------------------------- versioning

    /** The versioning level and where the clock and history stand. */
    @NotNull
    public JsonNode versioning() {
        return parse(NativeStore.nativeVersioning(handle));
    }

    /**
     * Changes the versioning level ({@code "off"}, {@code "stamped"}, {@code "log"});
     * {@code changeJson} is {@code {asOfIndex, stampIndex, allowLoss, author, message}}, or
     * {@code null}. Returns the new status.
     */
    @NotNull
    public JsonNode setVersioning(@NotNull String level, @Nullable String changeJson) {
        return parse(NativeStore.nativeSetVersioning(handle, level, changeJson));
    }

    /** Author and message (JSON, or {@code null} to clear) recorded on later writes' commits. */
    public void setCommitInfo(@Nullable String infoJson) {
        NativeStore.nativeSetCommitInfo(handle, infoJson);
    }

    /** The latest {@code limit} commits and level changes, newest first. */
    @NotNull
    public JsonNode history(long limit) {
        return parse(NativeStore.nativeHistory(handle, limit));
    }

    /** The changes after tick {@code after}: {@code [{"tick", "added", "quad"}]}. */
    @NotNull
    public JsonNode changes(long after) {
        return changes(after, null);
    }

    /** Like {@link #changes(long)}, but only up to tick {@code until}. */
    @NotNull
    public JsonNode changes(long after, @Nullable Long until) {
        return parse(NativeStore.nativeChanges(handle, after, until != null, until == null ? 0 : until));
    }

    /** The net difference between two versions: {@code [{"tick", "added", "quad"}]}. */
    @NotNull
    public JsonNode diff(@NotNull String from, @NotNull String to) {
        return parse(NativeStore.nativeDiff(handle, from, to));
    }

    /**
     * Removes the quads matching {@code patternJson} ({@code {subject, predicate, object,
     * graph}}, each optional) from the store and its whole history.
     */
    public void purge(@NotNull String patternJson, @Nullable String reason) {
        NativeStore.nativePurge(handle, patternJson, reason);
    }

    // ---------------------------------------------------------------------------- maintenance

    /** Refreshes planner statistics (run after large imports). */
    public void optimize() {
        NativeStore.nativeOptimize(handle);
    }

    public void clear() {
        NativeStore.nativeClear(handle);
    }

    @Override
    public void close() {
        if (closed.compareAndSet(false, true)) {
            NativeStore.nativeClose(handle);
        }
    }

    // -------------------------------------------------------------------------------- helpers

    private static JsonNode parse(String json) {
        try {
            return MAPPER.readTree(json);
        } catch (com.fasterxml.jackson.core.JsonProcessingException e) {
            throw new OxiliteException("the native layer returned malformed JSON", e);
        }
    }

    private static com.fasterxml.jackson.databind.node.ArrayNode quadsToJson(List<Quad> quads) {
        com.fasterxml.jackson.databind.node.ArrayNode array =
                com.fasterxml.jackson.databind.node.JsonNodeFactory.instance.arrayNode(quads.size());
        for (Quad q : quads) {
            array.add(Rdf.toJson(q));
        }
        return array;
    }
}
