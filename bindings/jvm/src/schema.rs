//! Schema registry native methods: declaring graphs as an ontology, SHACL shapes or a ShEx
//! schema, and the compiled SHACL property-shape index.

use crate::error::AppError;
use crate::handle;
use crate::jni_util::{get_opt_string, get_string, run_bool, run_long, run_string, run_unit};
use crate::store::{parse, required_graph};
use jni::objects::{JClass, JString};
use jni::sys::{jboolean, jlong, jstring};
use jni::JNIEnv;
use oxilite_core::StoreOptions;
use serde_json::Value;

#[no_mangle]
pub extern "system" fn Java_com_oxilitedb_oxilite_NativeStore_nativeRegisterSchemaGraph<'local>(
    mut env: JNIEnv<'local>,
    _class: JClass<'local>,
    handle: jlong,
    graph_json: JString<'local>,
    role: JString<'local>,
    registration: JString<'local>,
) {
    let graph_json = get_string(&mut env, &graph_json);
    let role = get_string(&mut env, &role);
    let registration = get_opt_string(&mut env, &registration);
    run_unit(&mut env, move || -> Result<(), AppError> {
        let backend = unsafe { handle::backend_ref(handle) };
        let graph = required_graph(&graph_json)?;
        let role: oxilite::schema::SchemaRole = role
            .parse()
            .map_err(|_| AppError::argument(format!("unknown schema role {role}")))?;
        let v: Value = registration
            .as_deref()
            .map(parse)
            .transpose()?
            .unwrap_or(Value::Null);
        let e = oxilite_core::json::schema_graph_from_json(graph.clone(), role, &v)
            .map_err(AppError::from)?;
        let r = oxilite::schema::Registration {
            iri: e.iri,
            version: e.version,
            sha256: e.sha256,
            imports: e.imports,
            applies_to: e.applies_to,
            active: e.active,
        };
        Ok(handle::with_store!(backend, s => s.register_schema_graph(&graph, role, &r))?)
    })
}

/// The registry as JSON: `[{graph, role, iri, version, sha256, imports, appliesTo, active,
/// loadedAt}]`.
#[no_mangle]
pub extern "system" fn Java_com_oxilitedb_oxilite_NativeStore_nativeSchemaGraphs<'local>(
    mut env: JNIEnv<'local>,
    _class: JClass<'local>,
    handle: jlong,
) -> jstring {
    run_string(&mut env, move || -> Result<String, AppError> {
        let backend = unsafe { handle::backend_ref(handle) };
        let rows = handle::with_store!(backend, s => s.schema_graphs())?;
        let v: Vec<Value> = rows
            .iter()
            .map(|e| oxilite_core::json::schema_graph_to_json(&e.to_entry()))
            .collect();
        Ok(Value::Array(v).to_string())
    })
}

#[no_mangle]
pub extern "system" fn Java_com_oxilitedb_oxilite_NativeStore_nativeSetSchemaGraphActive<'local>(
    mut env: JNIEnv<'local>,
    _class: JClass<'local>,
    handle: jlong,
    graph_json: JString<'local>,
    active: jboolean,
) -> jboolean {
    let graph_json = get_string(&mut env, &graph_json);
    let active = active == jni::sys::JNI_TRUE;
    run_bool(&mut env, move || -> Result<bool, AppError> {
        let backend = unsafe { handle::backend_ref(handle) };
        let graph = required_graph(&graph_json)?;
        Ok(handle::with_store!(backend, s => s.set_schema_graph_active(&graph, active))?)
    })
}

#[no_mangle]
pub extern "system" fn Java_com_oxilitedb_oxilite_NativeStore_nativeUnregisterSchemaGraph<
    'local,
>(
    mut env: JNIEnv<'local>,
    _class: JClass<'local>,
    handle: jlong,
    graph_json: JString<'local>,
) -> jboolean {
    let graph_json = get_string(&mut env, &graph_json);
    run_bool(&mut env, move || -> Result<bool, AppError> {
        let backend = unsafe { handle::backend_ref(handle) };
        let graph = required_graph(&graph_json)?;
        Ok(handle::with_store!(backend, s => s.unregister_schema_graph(&graph))?)
    })
}

#[no_mangle]
pub extern "system" fn Java_com_oxilitedb_oxilite_NativeStore_nativeDropSchemaGraph<'local>(
    mut env: JNIEnv<'local>,
    _class: JClass<'local>,
    handle: jlong,
    graph_json: JString<'local>,
) -> jlong {
    let graph_json = get_string(&mut env, &graph_json);
    run_long(&mut env, move || -> Result<i64, AppError> {
        let backend = unsafe { handle::backend_ref(handle) };
        let graph = required_graph(&graph_json)?;
        Ok(handle::with_store!(backend, s => s.drop_schema_graph(&graph))? as i64)
    })
}

#[no_mangle]
pub extern "system" fn Java_com_oxilitedb_oxilite_NativeStore_nativeShapeIndex<'local>(
    mut env: JNIEnv<'local>,
    _class: JClass<'local>,
    handle: jlong,
) -> jstring {
    run_string(&mut env, move || -> Result<String, AppError> {
        let backend = unsafe { handle::backend_ref(handle) };
        let i = handle::with_store!(backend, s => s.shape_index())?;
        Ok(oxilite_core::json::shape_index_to_json(&i).to_string())
    })
}

#[no_mangle]
pub extern "system" fn Java_com_oxilitedb_oxilite_NativeStore_nativeInstallSystemGraphs<'local>(
    mut env: JNIEnv<'local>,
    _class: JClass<'local>,
    handle: jlong,
) -> jboolean {
    run_bool(&mut env, move || -> Result<bool, AppError> {
        let backend = unsafe { handle::backend_ref(handle) };
        Ok(handle::with_store!(backend, s => s.install_system_graphs())?)
    })
}

/// The schema as a SQL script, given store options as JSON (static, no handle).
#[no_mangle]
pub extern "system" fn Java_com_oxilitedb_oxilite_NativeStore_nativeSchemaSql<'local>(
    mut env: JNIEnv<'local>,
    _class: JClass<'local>,
    options: JString<'local>,
) -> jstring {
    let options = get_opt_string(&mut env, &options);
    run_string(&mut env, move || -> Result<String, AppError> {
        let options: StoreOptions = options
            .as_deref()
            .map(parse)
            .transpose()?
            .unwrap_or_default();
        Ok(oxilite_core::schema::schema_sql(&options))
    })
}
