//! The opaque native handle a Java `NativeStore` holds: a boxed `Backend`, addressed by a
//! `long` field on the Java side (see `NativeStore.handle`).

use jni::sys::jlong;
use oxilite::blocking::Store;
use oxilite::dylib::DylibBackend;

pub enum Backend {
    Native(Store),
    Library(Store<DylibBackend>),
}

/// Runs `$body` with `$s` bound to the store, whatever its backend.
macro_rules! with_store {
    ($backend:expr, $s:ident => $body:expr) => {
        match $backend {
            $crate::handle::Backend::Native($s) => $body,
            $crate::handle::Backend::Library($s) => $body,
        }
    };
}
pub(crate) use with_store;

pub fn box_backend(backend: Backend) -> jlong {
    Box::into_raw(Box::new(backend)) as jlong
}

/// # Safety
/// `handle` must be a value previously returned by [`box_backend`] and not yet freed.
pub unsafe fn backend_ref<'a>(handle: jlong) -> &'a Backend {
    &*(handle as *const Backend)
}

/// # Safety
/// `handle` must be a value previously returned by [`box_backend`], and this must be the only
/// call that frees it (no further use of the handle after this call is safe).
pub unsafe fn drop_backend(handle: jlong) {
    if handle != 0 {
        drop(Box::from_raw(handle as *mut Backend));
    }
}
