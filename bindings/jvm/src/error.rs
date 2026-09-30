//! Maps every error this binding can raise to a Java exception, thrown via JNI.
//!
//! The class hierarchy (`com.oxilitedb.oxilite.exceptions`) mirrors the table in
//! `bindings/python`'s `store_error`: `OxiliteException` is the unchecked base, with
//! `OxiliteSyntaxException`, `OxiliteParseException`, `OxiliteBackendException` and
//! `OxiliteIOException` for the cases a caller may want to catch specifically, plus
//! `JsonLdException` (carries the JSON-LD error `code`) and the JDK's own
//! `UnsupportedOperationException`.

use jni::objects::{JObject, JThrowable, JValue};
use jni::JNIEnv;

const BASE: &str = "com/oxilitedb/oxilite/exceptions/OxiliteException";
const SYNTAX: &str = "com/oxilitedb/oxilite/exceptions/OxiliteSyntaxException";
const PARSE: &str = "com/oxilitedb/oxilite/exceptions/OxiliteParseException";
const BACKEND: &str = "com/oxilitedb/oxilite/exceptions/OxiliteBackendException";
const IO: &str = "com/oxilitedb/oxilite/exceptions/OxiliteIOException";
const UNSUPPORTED: &str = "java/lang/UnsupportedOperationException";
const JSON_LD: &str = "com/oxilitedb/oxilite/exceptions/JsonLdException";
const ILLEGAL_ARG: &str = "java/lang/IllegalArgumentException";

/// Every error this binding's native methods can produce.
pub enum AppError {
    Store(oxilite::Error),
    Cypher(oxilite::cypher::CypherError),
    Datalog(oxilite::datalog::DatalogError),
    Synalog(oxilite::synalog::SynalogError),
    JsonLd(oxilite::jsonld::JsonLdError),
    Parse(oxrdfio::RdfParseError),
    Json(serde_json::Error),
    /// A malformed argument that never reaches the store (bad enum string, missing field...).
    Argument(String),
}

impl From<oxilite::Error> for AppError {
    fn from(e: oxilite::Error) -> Self {
        AppError::Store(e)
    }
}
impl From<oxilite::cypher::CypherError> for AppError {
    fn from(e: oxilite::cypher::CypherError) -> Self {
        AppError::Cypher(e)
    }
}
impl From<oxilite::datalog::DatalogError> for AppError {
    fn from(e: oxilite::datalog::DatalogError) -> Self {
        AppError::Datalog(e)
    }
}
impl From<oxilite::synalog::SynalogError> for AppError {
    fn from(e: oxilite::synalog::SynalogError) -> Self {
        AppError::Synalog(e)
    }
}
impl From<oxilite::jsonld::JsonLdError> for AppError {
    fn from(e: oxilite::jsonld::JsonLdError) -> Self {
        AppError::JsonLd(e)
    }
}
impl From<oxrdfio::RdfParseError> for AppError {
    fn from(e: oxrdfio::RdfParseError) -> Self {
        AppError::Parse(e)
    }
}
impl From<serde_json::Error> for AppError {
    fn from(e: serde_json::Error) -> Self {
        AppError::Json(e)
    }
}

impl AppError {
    pub fn argument(msg: impl Into<String>) -> Self {
        AppError::Argument(msg.into())
    }

    fn store_class(e: &oxilite::Error) -> &'static str {
        use oxilite::Error as E;
        match e {
            E::Syntax(_) => SYNTAX,
            E::Parse(_) => PARSE,
            E::Backend(_) | E::Corrupted(_) | E::Collision(_) => BACKEND,
            E::Unsupported(_) => UNSUPPORTED,
            E::Io(_) => IO,
            E::Evaluation(_) | E::Other(_) => BASE,
        }
    }

    fn class(&self) -> &'static str {
        match self {
            AppError::Store(e) => Self::store_class(e),
            AppError::Cypher(e) => match e {
                oxilite::cypher::CypherError::Store(e) => Self::store_class(e),
                oxilite::cypher::CypherError::Syntax { .. } => SYNTAX,
                oxilite::cypher::CypherError::Unsupported(_) => UNSUPPORTED,
                oxilite::cypher::CypherError::Semantic(_)
                | oxilite::cypher::CypherError::Runtime(_)
                | oxilite::cypher::CypherError::ShapeViolation(_) => BASE,
            },
            AppError::Datalog(e) => match e {
                oxilite::datalog::DatalogError::Store(e) => Self::store_class(e),
                oxilite::datalog::DatalogError::Parse { .. } => SYNTAX,
                oxilite::datalog::DatalogError::Unsafe { .. }
                | oxilite::datalog::DatalogError::Unstratified { .. }
                | oxilite::datalog::DatalogError::NotTripleShaped { .. } => ILLEGAL_ARG,
                _ => BASE,
            },
            AppError::Synalog(e) => match e {
                oxilite::synalog::SynalogError::Store(e) => Self::store_class(e),
                oxilite::synalog::SynalogError::Parse(_)
                | oxilite::synalog::SynalogError::Pragma { .. } => SYNTAX,
                oxilite::synalog::SynalogError::Verify(_)
                | oxilite::synalog::SynalogError::Compile(_)
                | oxilite::synalog::SynalogError::UnknownPredicate { .. } => ILLEGAL_ARG,
                oxilite::synalog::SynalogError::Unsupported(_) => UNSUPPORTED,
            },
            AppError::JsonLd(e) => match e {
                oxilite::jsonld::JsonLdError::Store(e) => Self::store_class(e),
                _ => JSON_LD,
            },
            AppError::Parse(_) => PARSE,
            AppError::Json(_) => ILLEGAL_ARG,
            AppError::Argument(_) => ILLEGAL_ARG,
        }
    }

    /// The JSON-LD error code, when this wraps a JSON-LD processing error.
    fn json_ld_code(&self) -> Option<&str> {
        match self {
            AppError::JsonLd(e) => e.code(),
            _ => None,
        }
    }
}

impl std::fmt::Display for AppError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            AppError::Store(e) => write!(f, "{e}"),
            AppError::Cypher(e) => write!(f, "{e}"),
            AppError::Datalog(e) => write!(f, "{e}"),
            AppError::Synalog(e) => write!(f, "{e}"),
            AppError::JsonLd(e) => write!(f, "{e}"),
            AppError::Parse(e) => write!(f, "{e}"),
            AppError::Json(e) => write!(f, "{e}"),
            AppError::Argument(m) => write!(f, "{m}"),
        }
    }
}

/// Throws the Java exception matching `error`; does nothing if an exception is already pending
/// (e.g. raised while converting a Java string argument earlier in the same native call).
pub fn throw(env: &mut JNIEnv, error: AppError) {
    if env.exception_check().unwrap_or(true) {
        return;
    }
    let message = error.to_string();
    if error.class() == JSON_LD {
        throw_json_ld(env, &message, error.json_ld_code());
        return;
    }
    let _ = env.throw_new(error.class(), message);
}

fn throw_json_ld(env: &mut JNIEnv, message: &str, code: Option<&str>) {
    let mut build = || -> jni::errors::Result<JThrowable> {
        let class = env.find_class(JSON_LD)?;
        let jmessage = env.new_string(message)?;
        let jcode = match code {
            Some(c) => JObject::from(env.new_string(c)?),
            None => JObject::null(),
        };
        let obj = env.new_object(
            class,
            "(Ljava/lang/String;Ljava/lang/String;)V",
            &[JValue::Object(&jmessage), JValue::Object(&jcode)],
        )?;
        Ok(JThrowable::from(obj))
    };
    match build() {
        Ok(throwable) => {
            let _ = env.throw(throwable);
        }
        Err(_) => {
            let _ = env.throw_new(BASE, message);
        }
    }
}

/// Throws `message` as a plain `RuntimeException`, used only for a caught native panic.
pub fn throw_panic(env: &mut JNIEnv, message: &str) {
    if env.exception_check().unwrap_or(true) {
        return;
    }
    let _ = env.throw_new("java/lang/RuntimeException", format!("panic: {message}"));
}
