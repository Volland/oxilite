//! JVM bindings: `blocking::Store` (bundled SQLite or a dlopen'ed library) behind a small
//! JSON-in / JSON-out set of JNI native methods, wrapped by the Java `Store` class in
//! `java/src/main/java/com/oxilitedb/oxilite`. Usable directly from Java, Kotlin, Scala and
//! Clojure via ordinary Java interop.
//!
//! Terms, quads and results use the same JSON forms as the wasm core (`oxilite_core::json`)
//! and `bindings/node`/`bindings/python`, so every binding returns the same values (D37 in
//! `lat.md/decisions.md`). A store handle is a boxed `handle::Backend`, addressed by the Java
//! object's `long handle` field, since JNI cannot store a Rust struct in a Java object field
//! directly.
//!
// @lat: [[architecture#Bindings#JVM (Java/Kotlin/Scala/Clojure)]]

mod cypher;
mod datalog;
mod error;
mod handle;
mod jni_util;
mod jsonld;
mod schema;
mod store;
mod synalog;
mod versioning;
