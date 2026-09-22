//! Cloudflare D1 backend for oxilite in Rust Workers.
//!
//! ```ignore
//! let store = oxilite::AsyncStore::open(oxilite_d1::D1Backend::new(env.d1("DB")?)).await?;
//! ```
//!
//! Atomic requests run as one `batch()` (D1's only transaction); single reads use `raw()`.
//! 64-bit ids travel as TEXT because D1 returns JavaScript numbers.
//!
// @lat: [[architecture#Backends#Cloudflare D1]]

#[cfg(target_arch = "wasm32")]
mod d1 {
    use js_sys::{Array, Function, Promise, Reflect};
    use oxilite_core::sql::{Capabilities, Mode, Request, Response, ResultSet, SqlValue};
    use oxilite_core::{AsyncBackend, Error, Result};
    use serde_json::{Map, Value};
    use wasm_bindgen::{JsCast, JsValue};
    use wasm_bindgen_futures::JsFuture;
    use worker::D1Database;

    /// A D1 binding used as an oxilite backend.
    pub struct D1Backend {
        db: D1Database,
        caps: Capabilities,
    }

    impl D1Backend {
        pub fn new(db: D1Database) -> Self {
            Self::with_capabilities(db, Capabilities::d1())
        }

        pub fn with_capabilities(db: D1Database, caps: Capabilities) -> Self {
            Self { db, caps }
        }
    }

    fn value(v: Value) -> SqlValue {
        match v {
            Value::Null => SqlValue::Null,
            Value::Bool(b) => SqlValue::Integer(i64::from(b)),
            Value::Number(n) => match n.as_i64() {
                Some(i) => SqlValue::Integer(i),
                None => SqlValue::Real(n.as_f64().unwrap_or(f64::NAN)),
            },
            Value::String(s) => SqlValue::Text(s),
            other => SqlValue::Text(other.to_string()),
        }
    }

    fn err(e: worker::Error) -> Error {
        Error::backend(e)
    }

    fn js_err(e: JsValue) -> Error {
        let message = Reflect::get(&e, &"message".into())
            .ok()
            .and_then(|m| m.as_string())
            .unwrap_or_else(|| format!("{e:?}"));
        Error::backend(message)
    }

    impl AsyncBackend for D1Backend {
        async fn execute(&self, request: &Request) -> Result<Response> {
            if request.statements.is_empty() {
                return Ok(Vec::new());
            }
            if request.statements.iter().any(|s| !s.params.is_empty()) {
                return Err(Error::unsupported(
                    "bound parameters on D1 (oxilite inlines constants)",
                ));
            }
            if request.mode == Mode::Read && request.statements.len() == 1 {
                let rows: Vec<Vec<Value>> = self
                    .db
                    .prepare(&request.statements[0].sql)
                    .raw()
                    .await
                    .map_err(err)?;
                return Ok(vec![ResultSet {
                    rows: rows
                        .into_iter()
                        .map(|r| r.into_iter().map(value).collect())
                        .collect(),
                    changes: 0,
                }]);
            }
            // `worker::D1Result::meta()` deserializes `last_row_id` as an i64, which fails on
            // oxilite's 60-bit term ids (a JavaScript number above 2^53): read the batch results
            // through js-sys instead.
            let stmts: Array = request
                .statements
                .iter()
                .map(|s| JsValue::from(self.db.prepare(&s.sql).inner().clone()))
                .collect();
            let db: &JsValue = self.db.as_ref();
            let batch: Function = Reflect::get(db, &"batch".into())
                .map_err(js_err)?
                .unchecked_into();
            let promise: Promise = batch.call1(db, &stmts).map_err(js_err)?.unchecked_into();
            let results: Array = JsFuture::from(promise)
                .await
                .map_err(js_err)?
                .unchecked_into();
            results
                .iter()
                .map(|r| {
                    // Column order is the object key order (serde_json preserve_order).
                    let rows = Reflect::get(&r, &"results".into()).map_err(js_err)?;
                    let rows: Vec<Map<String, Value>> = if rows.is_undefined() || rows.is_null() {
                        Vec::new()
                    } else {
                        serde_wasm_bindgen::from_value(rows)
                            .map_err(|e| Error::backend(e.to_string()))?
                    };
                    let changes = Reflect::get(&r, &"meta".into())
                        .and_then(|m| Reflect::get(&m, &"changes".into()))
                        .ok()
                        .and_then(|c| c.as_f64())
                        .unwrap_or(0.0) as u64;
                    Ok(ResultSet {
                        rows: rows
                            .into_iter()
                            .map(|m| m.into_iter().map(|(_, v)| value(v)).collect())
                            .collect(),
                        changes,
                    })
                })
                .collect()
        }

        fn capabilities(&self) -> &Capabilities {
            &self.caps
        }
    }
}

#[cfg(target_arch = "wasm32")]
pub use d1::D1Backend;

/// The oxilite schema as a D1 migration script.
pub fn migration_sql(graph_index: bool) -> String {
    oxilite_core::schema::schema_sql(&oxilite_core::StoreOptions {
        graph_index,
        ..Default::default()
    })
}
