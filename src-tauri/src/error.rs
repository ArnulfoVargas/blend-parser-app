//! Errors the UI shows. The backend never writes the sentence: it sends a key
//! and its values, and the UI puts them in the user's language.

use serde::Serialize;
use serde_json::{json, Value};

#[derive(Debug, Serialize)]
pub struct AppError {
    /// Catalogue key under `error.`, such as `error.heightRange`.
    pub key: &'static str,
    /// Values the message names: a path, a limit, a name.
    pub params: Value,
}

impl AppError {
    pub fn new(key: &'static str) -> Self {
        Self { key, params: json!({}) }
    }

    pub fn with(key: &'static str, params: Value) -> Self {
        Self { key, params }
    }

    /// Something the user cannot fix from the UI; the detail stays as the system wrote it.
    pub fn internal(detail: impl std::fmt::Display) -> Self {
        Self::with("error.internal", json!({ "detail": detail.to_string() }))
    }

    /// A file that could not be read or written.
    pub fn file(path: &str, e: impl std::fmt::Display) -> Self {
        Self::with("error.file", json!({ "path": path, "detail": e.to_string() }))
    }
}

impl From<bp_blend::BlendError> for AppError {
    fn from(e: bp_blend::BlendError) -> Self {
        use bp_blend::BlendError as E;
        match e {
            E::BlenderNotFound => Self::new("error.blenderNotFound"),
            E::Io(e) => Self::with("error.io", json!({ "detail": e.to_string() })),
            E::Blender(log) => Self::with("error.blenderFailed", json!({ "detail": log })),
            E::Format(d) => Self::with("error.dumpFormat", json!({ "detail": d })),
            E::Empty => Self::new("error.noMeshes"),
        }
    }
}

impl<T> From<std::sync::PoisonError<T>> for AppError {
    fn from(_: std::sync::PoisonError<T>) -> Self {
        Self::new("error.stateLocked")
    }
}

impl From<tauri::Error> for AppError {
    fn from(e: tauri::Error) -> Self {
        Self::internal(e)
    }
}

pub type CmdResult<T> = Result<T, AppError>;

/// Fails with `key` unless `v` lies in `range`; the message names the limits.
pub fn in_range(v: f32, range: std::ops::RangeInclusive<f32>, key: &'static str) -> CmdResult<f32> {
    if v.is_finite() && range.contains(&v) {
        Ok(v)
    } else {
        Err(AppError::with(key, json!({ "min": range.start(), "max": range.end() })))
    }
}
