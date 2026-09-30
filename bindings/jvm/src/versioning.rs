//! Versioning native methods: the commit log, time-travel diffs, and purging history.

use crate::error::AppError;
use crate::handle;
use crate::jni_util::{get_opt_string, get_string, run_string, run_unit};
use crate::store::parse;
use jni::objects::{JClass, JString};
use jni::sys::{jlong, jstring};
use jni::JNIEnv;
use oxilite_core::json::{json_to_graph, json_to_term, quad_to_json};
use serde_json::Value;

fn changes_json(changes: &[oxilite::version::Change]) -> Value {
    Value::Array(
        changes
            .iter()
            .map(|c| serde_json::json!({"tick": c.tick, "added": c.added, "quad": quad_to_json(&c.quad)}))
            .collect(),
    )
}

#[no_mangle]
pub extern "system" fn Java_com_oxilitedb_oxilite_NativeStore_nativeVersioning<'local>(
    mut env: JNIEnv<'local>,
    _class: JClass<'local>,
    handle: jlong,
) -> jstring {
    run_string(&mut env, move || -> Result<String, AppError> {
        let backend = unsafe { handle::backend_ref(handle) };
        let s = handle::with_store!(backend, s => s.versioning())?;
        Ok(serde_json::to_string(&s)?)
    })
}

#[no_mangle]
pub extern "system" fn Java_com_oxilitedb_oxilite_NativeStore_nativeSetVersioning<'local>(
    mut env: JNIEnv<'local>,
    _class: JClass<'local>,
    handle: jlong,
    level: JString<'local>,
    change: JString<'local>,
) -> jstring {
    let level = get_string(&mut env, &level);
    let change = get_opt_string(&mut env, &change);
    run_string(&mut env, move || -> Result<String, AppError> {
        let backend = unsafe { handle::backend_ref(handle) };
        let change: oxilite::version::LevelChange = change
            .as_deref()
            .map(parse)
            .transpose()?
            .unwrap_or_default();
        let level = level
            .parse()
            .map_err(|_| AppError::argument(format!("unknown versioning level {level}")))?;
        let s = handle::with_store!(backend, s => s.set_versioning(level, change.clone()))?;
        Ok(serde_json::to_string(&s)?)
    })
}

#[no_mangle]
pub extern "system" fn Java_com_oxilitedb_oxilite_NativeStore_nativeSetCommitInfo<'local>(
    mut env: JNIEnv<'local>,
    _class: JClass<'local>,
    handle: jlong,
    info: JString<'local>,
) {
    let info = get_opt_string(&mut env, &info);
    run_unit(&mut env, move || -> Result<(), AppError> {
        let backend = unsafe { handle::backend_ref(handle) };
        let info: oxilite::version::CommitInfo =
            info.as_deref().map(parse).transpose()?.unwrap_or_default();
        handle::with_store!(backend, s => s.set_commit_info(info.clone()));
        Ok(())
    })
}

#[no_mangle]
pub extern "system" fn Java_com_oxilitedb_oxilite_NativeStore_nativeHistory<'local>(
    mut env: JNIEnv<'local>,
    _class: JClass<'local>,
    handle: jlong,
    limit: jlong,
) -> jstring {
    run_string(&mut env, move || -> Result<String, AppError> {
        let backend = unsafe { handle::backend_ref(handle) };
        let log = handle::with_store!(backend, s => s.history(limit as usize))?;
        Ok(serde_json::to_string(&log)?)
    })
}

#[no_mangle]
pub extern "system" fn Java_com_oxilitedb_oxilite_NativeStore_nativeChanges<'local>(
    mut env: JNIEnv<'local>,
    _class: JClass<'local>,
    handle: jlong,
    after: jlong,
    has_until: jni::sys::jboolean,
    until: jlong,
) -> jstring {
    let has_until = has_until == jni::sys::JNI_TRUE;
    run_string(&mut env, move || -> Result<String, AppError> {
        let backend = unsafe { handle::backend_ref(handle) };
        let until = if has_until { Some(until) } else { None };
        let c = handle::with_store!(backend, s => s.changes(after, until))?;
        Ok(changes_json(&c).to_string())
    })
}

#[no_mangle]
pub extern "system" fn Java_com_oxilitedb_oxilite_NativeStore_nativeDiff<'local>(
    mut env: JNIEnv<'local>,
    _class: JClass<'local>,
    handle: jlong,
    from: JString<'local>,
    to: JString<'local>,
) -> jstring {
    let from = get_string(&mut env, &from);
    let to = get_string(&mut env, &to);
    run_string(&mut env, move || -> Result<String, AppError> {
        let backend = unsafe { handle::backend_ref(handle) };
        let c = handle::with_store!(backend, s => s.diff(&from, &to))?;
        Ok(changes_json(&c).to_string())
    })
}

/// Removes the quads matching `pattern` (JSON terms `subject`, `predicate`, `object`, `graph`,
/// each optional) from the store and its whole history.
#[no_mangle]
pub extern "system" fn Java_com_oxilitedb_oxilite_NativeStore_nativePurge<'local>(
    mut env: JNIEnv<'local>,
    _class: JClass<'local>,
    handle: jlong,
    pattern: JString<'local>,
    reason: JString<'local>,
) {
    let pattern = get_string(&mut env, &pattern);
    let reason = get_opt_string(&mut env, &reason);
    run_unit(&mut env, move || -> Result<(), AppError> {
        let backend = unsafe { handle::backend_ref(handle) };
        let v: Value = parse(&pattern)?;
        let term = |k: &str| -> Result<Option<oxrdf::Term>, AppError> {
            v.get(k)
                .filter(|t| !t.is_null())
                .map(|t| json_to_term(t).map_err(AppError::from))
                .transpose()
        };
        let subject = match term("subject")? {
            None => None,
            Some(oxrdf::Term::NamedNode(n)) => Some(oxrdf::NamedOrBlankNode::NamedNode(n)),
            Some(oxrdf::Term::BlankNode(b)) => Some(oxrdf::NamedOrBlankNode::BlankNode(b)),
            Some(_) => {
                return Err(AppError::argument(
                    "a purge subject is an IRI or a blank node",
                ))
            }
        };
        let predicate = match term("predicate")? {
            None => None,
            Some(oxrdf::Term::NamedNode(n)) => Some(n),
            Some(_) => return Err(AppError::argument("a purge predicate is an IRI")),
        };
        let object = term("object")?;
        let graph_pattern = v
            .get("graph")
            .filter(|t| !t.is_null())
            .map(json_to_graph)
            .transpose()
            .map_err(AppError::from)?;
        Ok(handle::with_store!(backend, s => s.purge(
            subject.as_ref().map(Into::into),
            predicate.as_ref().map(Into::into),
            object.as_ref().map(Into::into),
            graph_pattern.as_ref().map(Into::into),
            reason.as_deref(),
        ))?)
    })
}
