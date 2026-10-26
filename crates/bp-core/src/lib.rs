//! BlendParser core: turns a colour-textured scene into Bambu Studio filament
//! painting, with the operations that make it print faster. No I/O, no UI.

pub mod bvh;
pub mod color;
pub mod cut;
pub mod encode;
pub mod estimate;
pub mod field;
pub mod geom;
pub mod mesh;
pub mod optimize;
pub mod paint;
pub mod palette;
pub mod printability;
pub mod project;
pub mod region;
pub mod scene;
pub mod separate;
pub mod split;
pub mod waste;

pub use mesh::{MeshOps, RepairReport};
pub use project::{
    ColorOps, CutProposal, DetectedPiece, ExportPart, Mat3, Preview, PrintOps, Project, RegionAction, RegionFailure,
    RegionPreview, SeparateError, Stats, IDENTITY,
};
pub use scene::{ColorSource, Image, MaterialInfo, Part, Scene};
pub use split::ColorPiece;
pub use waste::{Waste, WasteParams};
