//! Core store native methods: construction, SPARQL query/update, RDF load/dump, quad-level
//! CRUD and maintenance. Every JSON-carrying argument/result uses `oxilite_core::json`, the
//! same wire format as `bindings/node` and `bindings/python` (see the crate doc comment).

use crate::error::AppError;
use crate::handle::{self, Backend};
use crate::jni_util::{
    get_opt_string, get_string, run_bool, run_create, run_long, run_string, run_unit,
};
use jni::objects::{JClass, JString};
use jni::sys::{jboolean, jlong, jstring, JNI_TRUE};
use jni::JNIEnv;
use oxilite::blocking::Store;
use oxilite::dylib::DylibBackend;
use oxilite_core::json::{
    json_to_graph, json_to_quad, json_to_term, output_to_format, output_to_json, quad_to_json,
    JsQueryOptions,
};
use oxilite_core::StoreOptions;
use oxrdf::{GraphName, NamedOrBlankNode, Quad, Term};
use oxrdfio::{RdfFormat, RdfParser, RdfSerializer};
use serde_json::{json, Value};
use spargebra::SparqlParser;

pub(crate) fn parse<T: serde::de::DeserializeOwned>(s: &str) -> Result<T, AppError> {
    Ok(serde_json::from_str(s)?)
}

pub(crate) fn format(name: &str) -> Result<RdfFormat, AppError> {
    RdfFormat::from_media_type(name)
        .or_else(|| RdfFormat::from_extension(name))
        .ok_or_else(|| AppError::argument(format!("unknown RDF format {name}")))
}

pub(crate) fn graph(json: Option<String>) -> Result<Option<GraphName>, AppError> {
    json.map(|g| json_to_graph(&parse::<Value>(&g)?).map_err(AppError::from))
        .transpose()
}

pub(crate) fn required_graph(json: &str) -> Result<GraphName, AppError> {
    json_to_graph(&parse::<Value>(json)?).map_err(AppError::from)
}

#[no_mangle]
pub extern "system" fn Java_com_oxilitedb_oxilite_NativeStore_nativeCreate<'local>(
    mut env: JNIEnv<'local>,
    _class: JClass<'local>,
    path: JString<'local>,
    library: JString<'local>,
    options: JString<'local>,
) -> jlong {
    let path = get_opt_string(&mut env, &path);
    let library = get_opt_string(&mut env, &library);
    let options = get_opt_string(&mut env, &options);
    run_create(&mut env, move || -> Result<jlong, AppError> {
        let options: StoreOptions = options
            .as_deref()
            .map(parse)
            .transpose()?
            .unwrap_or_default();
        let backend = match library {
            Some(lib) => Backend::Library(Store::with_backend_and_options(
                DylibBackend::open(lib, path.as_deref().unwrap_or(":memory:"))
                    .map_err(oxilite::Error::backend)?,
                &options,
            )?),
            None => Backend::Native(match path {
                Some(p) => Store::open_with_options(p, options)?,
                None => Store::with_backend_and_options(
                    oxilite::rusqlite::RusqliteBackend::memory()
                        .map_err(oxilite::Error::backend)?,
                    &options,
                )?,
            }),
        };
        Ok(handle::box_backend(backend))
    })
}

#[no_mangle]
pub extern "system" fn Java_com_oxilitedb_oxilite_NativeStore_nativeClose<'local>(
    _env: JNIEnv<'local>,
    _class: JClass<'local>,
    handle: jlong,
) {
    // Safety: `handle` came from `nativeCreate` and `close()` on the Java side guards against
    // calling this more than once for the same handle.
    unsafe { handle::drop_backend(handle) };
}

#[no_mangle]
pub extern "system" fn Java_com_oxilitedb_oxilite_NativeStore_nativeQuery<'local>(
    mut env: JNIEnv<'local>,
    _class: JClass<'local>,
    handle: jlong,
    sparql: JString<'local>,
    options: JString<'local>,
) -> jstring {
    let sparql = get_string(&mut env, &sparql);
    let options = get_opt_string(&mut env, &options);
    run_string(&mut env, move || -> Result<String, AppError> {
        let backend = unsafe { handle::backend_ref(handle) };
        let options: JsQueryOptions = options
            .as_deref()
            .map(parse)
            .transpose()?
            .unwrap_or_default();
        let mut parser = SparqlParser::new();
        if let Some(b) = &options.base_iri {
            parser = parser
                .with_base_iri(b)
                .map_err(|e| AppError::argument(e.to_string()))?;
        }
        let q = parser
            .parse_query(&sparql)
            .map_err(|e| AppError::Store(oxilite::Error::Syntax(e)))?;
        let core = options
            .to_options()
            .map_err(|e| AppError::argument(e.to_string()))?;
        let out = handle::with_store!(backend, s => s.query_output(q, &core))?;
        let v = match &options.results_format {
            Some(f) => {
                json!({"kind": "text", "value": output_to_format(&out, f).map_err(AppError::from)?})
            }
            None => output_to_json(&out),
        };
        Ok(v.to_string())
    })
}

#[no_mangle]
pub extern "system" fn Java_com_oxilitedb_oxilite_NativeStore_nativeExplain<'local>(
    mut env: JNIEnv<'local>,
    _class: JClass<'local>,
    handle: jlong,
    sparql: JString<'local>,
) -> jstring {
    let sparql = get_string(&mut env, &sparql);
    run_string(&mut env, move || -> Result<String, AppError> {
        let backend = unsafe { handle::backend_ref(handle) };
        Ok(handle::with_store!(backend, s => s.explain(sparql.as_str()))?)
    })
}

#[no_mangle]
pub extern "system" fn Java_com_oxilitedb_oxilite_NativeStore_nativeUpdate<'local>(
    mut env: JNIEnv<'local>,
    _class: JClass<'local>,
    handle: jlong,
    sparql: JString<'local>,
    base_iri: JString<'local>,
) {
    let sparql = get_string(&mut env, &sparql);
    let base_iri = get_opt_string(&mut env, &base_iri);
    run_unit(&mut env, move || -> Result<(), AppError> {
        let backend = unsafe { handle::backend_ref(handle) };
        let mut parser = SparqlParser::new();
        if let Some(b) = base_iri {
            parser = parser
                .with_base_iri(b)
                .map_err(|e| AppError::argument(e.to_string()))?;
        }
        let u = parser
            .parse_update(&sparql)
            .map_err(|e| AppError::Store(oxilite::Error::Syntax(e)))?;
        Ok(handle::with_store!(backend, s => s.update(u))?)
    })
}

#[no_mangle]
pub extern "system" fn Java_com_oxilitedb_oxilite_NativeStore_nativeExplainUpdate<'local>(
    mut env: JNIEnv<'local>,
    _class: JClass<'local>,
    handle: jlong,
    sparql: JString<'local>,
) -> jstring {
    let sparql = get_string(&mut env, &sparql);
    run_string(&mut env, move || -> Result<String, AppError> {
        let backend = unsafe { handle::backend_ref(handle) };
        Ok(handle::with_store!(backend, s => s.explain_update(sparql.as_str()))?)
    })
}

#[no_mangle]
#[allow(clippy::too_many_arguments)]
pub extern "system" fn Java_com_oxilitedb_oxilite_NativeStore_nativeLoad<'local>(
    mut env: JNIEnv<'local>,
    _class: JClass<'local>,
    handle: jlong,
    data: JString<'local>,
    format_name: JString<'local>,
    base_iri: JString<'local>,
    to_graph: JString<'local>,
    bulk: jboolean,
) {
    let data = get_string(&mut env, &data);
    let format_name = get_string(&mut env, &format_name);
    let base_iri = get_opt_string(&mut env, &base_iri);
    let to_graph = get_opt_string(&mut env, &to_graph);
    let bulk = bulk == JNI_TRUE;
    run_unit(&mut env, move || -> Result<(), AppError> {
        let backend = unsafe { handle::backend_ref(handle) };
        let mut parser = RdfParser::from_format(format(&format_name)?);
        if let Some(b) = base_iri {
            parser = parser
                .with_base_iri(b)
                .map_err(|e| AppError::argument(e.to_string()))?;
        }
        if let Some(g) = graph(to_graph)? {
            parser = parser.with_default_graph(g);
        }
        handle::with_store!(backend, s => if bulk {
            let mut loader = s.bulk_loader();
            match loader.load_from_slice(parser, data.as_bytes()) {
                Ok(()) => loader.commit(),
                Err(e) => Err(e),
            }
        } else {
            s.load_from_slice(parser, data.as_bytes())
        })?;
        Ok(())
    })
}

#[no_mangle]
pub extern "system" fn Java_com_oxilitedb_oxilite_NativeStore_nativeAdd<'local>(
    mut env: JNIEnv<'local>,
    _class: JClass<'local>,
    handle: jlong,
    quads: JString<'local>,
) {
    let quads = get_string(&mut env, &quads);
    run_unit(&mut env, move || -> Result<(), AppError> {
        let backend = unsafe { handle::backend_ref(handle) };
        let quads = parse::<Vec<Value>>(&quads)?
            .iter()
            .map(json_to_quad)
            .collect::<oxilite_core::Result<Vec<Quad>>>()?;
        Ok(handle::with_store!(backend, s => s.extend(quads))?)
    })
}

#[no_mangle]
pub extern "system" fn Java_com_oxilitedb_oxilite_NativeStore_nativeDelete<'local>(
    mut env: JNIEnv<'local>,
    _class: JClass<'local>,
    handle: jlong,
    quads: JString<'local>,
) {
    let quads = get_string(&mut env, &quads);
    run_unit(&mut env, move || -> Result<(), AppError> {
        let backend = unsafe { handle::backend_ref(handle) };
        for q in parse::<Vec<Value>>(&quads)? {
            let q = json_to_quad(&q)?;
            handle::with_store!(backend, s => s.remove(&q))?;
        }
        Ok(())
    })
}

#[no_mangle]
pub extern "system" fn Java_com_oxilitedb_oxilite_NativeStore_nativeHas<'local>(
    mut env: JNIEnv<'local>,
    _class: JClass<'local>,
    handle: jlong,
    quad: JString<'local>,
) -> jboolean {
    let quad = get_string(&mut env, &quad);
    run_bool(&mut env, move || -> Result<bool, AppError> {
        let backend = unsafe { handle::backend_ref(handle) };
        let q = json_to_quad(&parse(&quad)?)?;
        Ok(handle::with_store!(backend, s => s.contains(&q))?)
    })
}

#[no_mangle]
pub extern "system" fn Java_com_oxilitedb_oxilite_NativeStore_nativeMatch<'local>(
    mut env: JNIEnv<'local>,
    _class: JClass<'local>,
    handle: jlong,
    subject: JString<'local>,
    predicate: JString<'local>,
    object: JString<'local>,
    graph_name: JString<'local>,
) -> jstring {
    let subject = get_opt_string(&mut env, &subject);
    let predicate = get_opt_string(&mut env, &predicate);
    let object = get_opt_string(&mut env, &object);
    let graph_name = get_opt_string(&mut env, &graph_name);
    run_string(&mut env, move || -> Result<String, AppError> {
        let backend = unsafe { handle::backend_ref(handle) };
        let term = |t: Option<String>| -> Result<Option<Term>, AppError> {
            t.map(|t| json_to_term(&parse(&t)?).map_err(AppError::from))
                .transpose()
        };
        let s = match term(subject)? {
            None => None,
            Some(Term::NamedNode(n)) => Some(NamedOrBlankNode::from(n)),
            Some(Term::BlankNode(b)) => Some(NamedOrBlankNode::from(b)),
            Some(_) => return Ok(json!({"kind": "quads", "quads": []}).to_string()),
        };
        let p = match term(predicate)? {
            None => None,
            Some(Term::NamedNode(n)) => Some(n),
            Some(_) => return Ok(json!({"kind": "quads", "quads": []}).to_string()),
        };
        let o = term(object)?;
        let g = graph(graph_name)?;
        let quads = handle::with_store!(backend, st => st
            .quads_for_pattern(s.as_ref().map(Into::into), p.as_ref().map(Into::into), o.as_ref().map(Into::into), g.as_ref().map(Into::into))
            .collect::<oxilite_core::Result<Vec<Quad>>>())?;
        Ok(
            json!({"kind": "quads", "quads": quads.iter().map(quad_to_json).collect::<Vec<_>>()})
                .to_string(),
        )
    })
}

#[no_mangle]
pub extern "system" fn Java_com_oxilitedb_oxilite_NativeStore_nativeSize<'local>(
    mut env: JNIEnv<'local>,
    _class: JClass<'local>,
    handle: jlong,
) -> jlong {
    run_long(&mut env, move || -> Result<i64, AppError> {
        let backend = unsafe { handle::backend_ref(handle) };
        Ok(handle::with_store!(backend, s => s.len())? as i64)
    })
}

#[no_mangle]
pub extern "system" fn Java_com_oxilitedb_oxilite_NativeStore_nativeDump<'local>(
    mut env: JNIEnv<'local>,
    _class: JClass<'local>,
    handle: jlong,
    format_name: JString<'local>,
    from_graph: JString<'local>,
) -> jstring {
    let format_name = get_string(&mut env, &format_name);
    let from_graph = get_opt_string(&mut env, &from_graph);
    run_string(&mut env, move || -> Result<String, AppError> {
        let backend = unsafe { handle::backend_ref(handle) };
        let serializer = RdfSerializer::from_format(format(&format_name)?);
        let bytes = match graph(from_graph)? {
            Some(g) => {
                handle::with_store!(backend, s => s.dump_graph_to_writer(&g, serializer, Vec::new()))
            }
            None => handle::with_store!(backend, s => s.dump_to_writer(serializer, Vec::new())),
        }?;
        String::from_utf8(bytes).map_err(|e| AppError::argument(e.to_string()))
    })
}

#[no_mangle]
pub extern "system" fn Java_com_oxilitedb_oxilite_NativeStore_nativeOptimize<'local>(
    mut env: JNIEnv<'local>,
    _class: JClass<'local>,
    handle: jlong,
) {
    run_unit(&mut env, move || -> Result<(), AppError> {
        let backend = unsafe { handle::backend_ref(handle) };
        Ok(handle::with_store!(backend, s => s.optimize())?)
    })
}

#[no_mangle]
pub extern "system" fn Java_com_oxilitedb_oxilite_NativeStore_nativeClear<'local>(
    mut env: JNIEnv<'local>,
    _class: JClass<'local>,
    handle: jlong,
) {
    run_unit(&mut env, move || -> Result<(), AppError> {
        let backend = unsafe { handle::backend_ref(handle) };
        Ok(handle::with_store!(backend, s => s.clear())?)
    })
}

/// Computes the OWL 2 RL closure into the inference table, with SQL rules or (`reasonable`) in
/// memory; returns the number of inferred triples.
#[no_mangle]
pub extern "system" fn Java_com_oxilitedb_oxilite_NativeStore_nativeMaterialize<'local>(
    mut env: JNIEnv<'local>,
    _class: JClass<'local>,
    handle: jlong,
    reasonable: jboolean,
) -> jlong {
    let reasonable = reasonable == JNI_TRUE;
    run_long(&mut env, move || -> Result<i64, AppError> {
        let backend = unsafe { handle::backend_ref(handle) };
        let n = if reasonable {
            handle::with_store!(backend, s => s.materialize_with_reasonable())
        } else {
            handle::with_store!(backend, s => s.materialize())
        };
        Ok(n? as i64)
    })
}

/// Removes every materialized inference.
#[no_mangle]
pub extern "system" fn Java_com_oxilitedb_oxilite_NativeStore_nativeClearInferences<'local>(
    mut env: JNIEnv<'local>,
    _class: JClass<'local>,
    handle: jlong,
) {
    run_unit(&mut env, move || -> Result<(), AppError> {
        let backend = unsafe { handle::backend_ref(handle) };
        Ok(handle::with_store!(backend, s => s.clear_inferences())?)
    })
}

#[no_mangle]
pub extern "system" fn Java_com_oxilitedb_oxilite_NativeStore_nativeBackup<'local>(
    mut env: JNIEnv<'local>,
    _class: JClass<'local>,
    handle: jlong,
    path: JString<'local>,
) {
    let path = get_string(&mut env, &path);
    run_unit(&mut env, move || -> Result<(), AppError> {
        let backend = unsafe { handle::backend_ref(handle) };
        Ok(handle::with_store!(backend, s => s.backup(path))?)
    })
}
