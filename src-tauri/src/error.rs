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
            E::Unsupported(path) => Self::with("error.notBlend", json!({ "name": path })),
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

impl From<bp_core::RegionFailure> for AppError {
    fn from(e: bp_core::RegionFailure) -> Self {
        use bp_core::region::RegionError as R;
        use bp_core::RegionFailure as F;
        match e {
            F::Region(R::Empty) => Self::new("error.regionEmpty"),
            F::Region(R::AmbiguousBorder) => Self::new("error.regionBorder"),
            F::Region(R::PinchedBorder) => Self::new("error.regionPinched"),
            F::Region(R::AmbiguousCap) => Self::new("error.regionCap"),
            F::Region(R::ThinWall { count }) => Self::with("error.regionThin", json!({ "count": count })),
            F::Region(R::Folded { count }) => Self::with("error.regionFolded", json!({ "count": count })),
            F::Region(R::Clearance { count }) => Self::with("error.regionClearance", json!({ "count": count })),
            F::Region(R::Overlap) => Self::new("error.regionOverlap"),
            F::Everything => Self::new("error.regionEverything"),
            F::Flat => Self::new("error.regionFlat"),
        }
    }
}

impl From<bp_core::RefineFailure> for AppError {
    fn from(e: bp_core::RefineFailure) -> Self {
        match e {
            bp_core::RefineFailure::Reshaped => Self::new("error.refineReshaped"),
            bp_core::RefineFailure::Nothing => Self::new("error.refineNothing"),
        }
    }
}
