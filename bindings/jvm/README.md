<p align="center">
  <a href="https://oxilitedb.com"><img src="https://raw.githubusercontent.com/Volland/oxilite/main/site/assets/logo.png" alt="oxilite" width="120"></a>
</p>

# oxilite (JVM)

**An Oxigraph-compatible SPARQL 1.1 store for the JVM, on SQLite.** A JNI binding around the
same `oxilite` Rust store used by `@oxilite/node` and the `oxilite` Python package: SPARQL
1.1 query and update, **openCypher**, **Datalog**, **Synalog**, RDFS/OWL reasoning, JSON-LD
and Verifiable Credentials, a schema registry, and versioning — on a single SQLite file.

One artifact, `com.oxilitedb:oxilite-jvm`, works directly from **Java**, **Kotlin**, **Scala**
and **Clojure**: they all compile to JVM bytecode and call the same Java API through ordinary
interop, so there is nothing language-specific to install beyond the jar.

**[Website](https://oxilitedb.com)** · **[Guide and architecture](https://github.com/Volland/oxilite#readme)** · [Changelog and issues](https://github.com/Volland/oxilite/issues)

```xml
<dependency>
  <groupId>com.oxilitedb</groupId>
  <artifactId>oxilite-jvm</artifactId>
  <version>0.9.1</version>
</dependency>
```

## Quick start

Every RDF-carrying argument/result uses the typed classes in `com.oxilitedb.oxilite.model`
(`NamedNode`, `BlankNode`, `Literal`, `Quad`, ...); query/Cypher/Datalog/Synalog/JSON-LD
results and options are Jackson `JsonNode`s / JSON strings, the same wire format
`@oxilite/node` and the Python bindings use.

### Java

```java
import com.oxilitedb.oxilite.Store;
import com.oxilitedb.oxilite.model.*;
import com.fasterxml.jackson.databind.JsonNode;

try (Store store = new Store("data.sqlite")) {   // new Store() for an in-memory store
    store.load("""
        @prefix ex: <http://example.com/> .
        ex:ada a ex:Person ; ex:name "Ada" ; ex:knows ex:alan .
        """, "text/turtle");

    NamedNode alan = new NamedNode("http://example.com/alan");
    store.add(new Quad(alan, new NamedNode("http://example.com/name"), new Literal("Alan")));

    JsonNode result = store.query("SELECT ?name WHERE { ?p <http://example.com/name> ?name }");
    for (JsonNode row : result.get("rows")) {
        System.out.println(row.get(0).get("value").asText());
    }
}
```

### Kotlin

```kotlin
import com.oxilitedb.oxilite.Store
import com.oxilitedb.oxilite.model.NamedNode

Store("data.sqlite").use { store ->
    store.update("INSERT DATA { <http://example.com/ada> a <http://example.com/Person> }")
    val result = store.query("ASK { <http://example.com/ada> a <http://example.com/Person> }")
    println(result["value"].asBoolean())
}
```

`Store` implements `AutoCloseable`, so Kotlin's `use { }` closes it deterministically, same as
Java's try-with-resources.

### Scala

```scala
import com.oxilitedb.oxilite.Store
import com.oxilitedb.oxilite.model.{NamedNode, Literal, Quad}

val store = new Store()
try {
  store.add(new Quad(
    new NamedNode("http://example.com/ada"),
    new NamedNode("http://example.com/name"),
    new Literal("Ada")))
  println(store.size())
} finally {
  store.close()
}
```

### Clojure

```clojure
(import '(com.oxilitedb.oxilite Store)
        '(com.oxilitedb.oxilite.model NamedNode Literal Quad))

(with-open [store (Store.)]
  (.add store (Quad. (NamedNode. "http://example.com/ada")
                      (NamedNode. "http://example.com/name")
                      (Literal. "Ada")))
  (println (.size store)))
```

## Building from source

```bash
scripts/build-native.sh [--release]   # builds the Rust cdylib, copies it into java/src/main/resources
cd java && mvn test
```

`NativeLoader` extracts the native library bundled under `native/<os>-<arch>/` in the jar's
resources to a temp file and `System.load()`s it, the standard approach for a JNI library
shipped inside a jar (as `sqlite-jdbc` and `grpc-netty` do). This first release bundles
`darwin-aarch64`; `scripts/build-native.sh` on another platform/CI leg adds that platform's
native library to the same jar.

## Exceptions

Every native method throws an unchecked `com.oxilitedb.oxilite.exceptions.OxiliteException`
(or a subtype — `OxiliteSyntaxException`, `OxiliteParseException`, `OxiliteBackendException`,
`OxiliteIOException`, `JsonLdException`, or the JDK's own `UnsupportedOperationException`)
instead of a checked exception, so call sites don't need `throws` clauses.

## License

MIT OR Apache-2.0, same as the rest of oxilite.
