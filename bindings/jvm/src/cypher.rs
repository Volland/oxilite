//! openCypher native methods: statement execution and its SQL/SPARQL explanation.

use crate::error::AppError;
use crate::handle::{self};
use crate::jni_util::{get_opt_string, get_string, run_string};
use jni::objects::{JClass, JString};
use jni::sys::{jlong, jstring};
use jni::JNIEnv;
use serde_json::json;

pub(crate) fn cypher_args(
    params: Option<String>,
    options: Option<String>,
) -> Result<(oxilite::cypher::Params, oxilite::cypher::CypherOptions), AppError> {
    let params = match params {
        Some(p) => oxilite::cypher::json::params_from_json(&p).map_err(AppError::from)?,
        None => oxilite::cypher::Params::new(),
    };
    let opts = match options {
        Some(o) => oxilite::cypher::json::options_from_json(&o).map_err(AppError::from)?,
        None => oxilite::cypher::CypherOptions::default(),
    };
    Ok((params, opts))
}

#[no_mangle]
pub extern "system" fn Java_com_oxilitedb_oxilite_NativeStore_nativeCypher<'local>(
    mut env: JNIEnv<'local>,
    _class: JClass<'local>,
    handle: jlong,
    query: JString<'local>,
    params: JString<'local>,
    options: JString<'local>,
) -> jstring {
    let query = get_string(&mut env, &query);
    let params = get_opt_string(&mut env, &params);
    let options = get_opt_string(&mut env, &options);
    run_string(&mut env, move || -> Result<String, AppError> {
        let backend = unsafe { handle::backend_ref(handle) };
        let (params, opts) = cypher_args(params, options)?;
        let r = handle::with_store!(backend, s => s.cypher_with(&query, &params, &opts))?;
        let mut v = r.to_json();
        v["kind"] = json!("cypher");
        Ok(v.to_string())
    })
}

#[no_mangle]
pub extern "system" fn Java_com_oxilitedb_oxilite_NativeStore_nativeExplainCypher<'local>(
    mut env: JNIEnv<'local>,
    _class: JClass<'local>,
    handle: jlong,
    query: JString<'local>,
    params: JString<'local>,
    options: JString<'local>,
) -> jstring {
    let query = get_string(&mut env, &query);
    let params = get_opt_string(&mut env, &params);
    let options = get_opt_string(&mut env, &options);
    run_string(&mut env, move || -> Result<String, AppError> {
        let backend = unsafe { handle::backend_ref(handle) };
        let (params, opts) = cypher_args(params, options)?;
        Ok(handle::with_store!(backend, s => s.explain_cypher(&query, &params, &opts))?)
    })
}
