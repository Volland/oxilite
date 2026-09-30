//! Datalog native methods: recursive rules with stratified negation, constraints and
//! aggregation, plus materializing derived facts alongside OWL inferences.

use crate::error::AppError;
use crate::handle;
use crate::jni_util::{get_opt_string, get_string, run_string};
use jni::objects::{JClass, JString};
use jni::sys::{jlong, jstring};
use jni::JNIEnv;

pub(crate) fn datalog_args(options: Option<String>) -> Result<oxilite::datalog::Options, AppError> {
    match options {
        Some(o) => oxilite::datalog::json::options_from_json(&serde_json::from_str::<
            serde_json::Value,
        >(&o)?)
        .map_err(AppError::from),
        None => oxilite::datalog::json::options_from_json(&serde_json::Value::Null)
            .map_err(AppError::from),
    }
}

#[no_mangle]
pub extern "system" fn Java_com_oxilitedb_oxilite_NativeStore_nativeDatalog<'local>(
    mut env: JNIEnv<'local>,
    _class: JClass<'local>,
    handle: jlong,
    program: JString<'local>,
    options: JString<'local>,
) -> jstring {
    let program = get_string(&mut env, &program);
    let options = get_opt_string(&mut env, &options);
    run_string(&mut env, move || -> Result<String, AppError> {
        let backend = unsafe { handle::backend_ref(handle) };
        let opts = datalog_args(options)?;
        let r = handle::with_store!(backend, s => s.datalog_with(&program, &opts))?;
        Ok(oxilite::datalog::json::result_to_json(&r).to_string())
    })
}

#[no_mangle]
pub extern "system" fn Java_com_oxilitedb_oxilite_NativeStore_nativeDatalogMaterialize<'local>(
    mut env: JNIEnv<'local>,
    _class: JClass<'local>,
    handle: jlong,
    program: JString<'local>,
    options: JString<'local>,
) -> jstring {
    let program = get_string(&mut env, &program);
    let options = get_opt_string(&mut env, &options);
    run_string(&mut env, move || -> Result<String, AppError> {
        let backend = unsafe { handle::backend_ref(handle) };
        let opts = datalog_args(options)?;
        let s = handle::with_store!(backend, s => s.datalog_materialize_with(&program, &opts))?;
        Ok(oxilite::datalog::json::stats_to_json(&s).to_string())
    })
}

#[no_mangle]
pub extern "system" fn Java_com_oxilitedb_oxilite_NativeStore_nativeExplainDatalog<'local>(
    mut env: JNIEnv<'local>,
    _class: JClass<'local>,
    handle: jlong,
    program: JString<'local>,
) -> jstring {
    let program = get_string(&mut env, &program);
    run_string(&mut env, move || -> Result<String, AppError> {
        let backend = unsafe { handle::backend_ref(handle) };
        Ok(handle::with_store!(backend, s => s.explain_datalog(&program))?)
    })
}
