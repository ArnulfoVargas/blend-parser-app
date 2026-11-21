//! BlendParser core: turns a colour-textured scene into Bambu Studio filament
//! painting, with the operations that make it print faster. No I/O, no UI.

pub mod bvh;
pub mod color;
pub mod cut;
pub mod encode;
pub mod estimate;
pub mod field;
pub mod geom;
pub mod kit;
pub mod mesh;
pub mod optimize;
pub mod paint;
pub mod palette;
pub mod printability;
pub mod project;
pub mod render;
pub mod scene;
pub mod snapshot;
pub mod split;
pub mod waste;

pub use kit::Kit;
pub use mesh::{MeshOps, RepairReport};
pub use project::{ColorOps, ExportPart, Mat3, Preview, PrintOps, Project, Stats, IDENTITY};
pub use scene::{ColorSource, Image, MaterialInfo, Part, Scene};
pub use snapshot::SnapshotError;
pub use split::{Piece, PinJoin, PinParams, Split, SplitParams};
pub use waste::{Waste, WasteParams};
