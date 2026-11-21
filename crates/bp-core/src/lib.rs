//! BlendParser core: turns a colour-textured scene into Bambu Studio filament
//! painting, with the operations that make it print faster. No I/O, no UI.

pub mod bvh;
pub mod color;
pub mod connect;
pub mod cut;
pub mod encode;
pub mod estimate;
pub mod field;
pub mod fit;
pub mod geom;
pub mod hollow;
pub mod kit;
pub mod mesh;
pub mod optimize;
pub mod paint;
pub mod palette;
pub mod printability;
pub mod project;
pub mod refine;
pub mod region;
pub mod render;
pub mod scene;
pub mod separate;
pub mod smooth;
pub mod snapshot;
pub mod split;
pub mod waste;

pub use kit::Kit;
pub use mesh::{MeshIssues, MeshOps, RepairReport};
pub use project::{
    CloneReport, ColorOps, ColourMatch, ConnectError, CutProposal, DetectedPiece, ExportPart, Mat3, OrientationPlan,
    PlateEditError, PlateGroup, Preview, PrintOps, Project, RefineFailure, RegionAction, RegionFailure, RegionPreview,
    SeparateError, SplitError, SplitPiece, SplitPlan, SplitReport, Stats, SuggestionKind, IDENTITY,
};
pub use scene::{ColorSource, Image, MaterialInfo, Part, Scene};
pub use snapshot::SnapshotError;
pub use split::{Piece, PinJoin, PinParams, Split, SplitParams};
pub use waste::{Waste, WasteParams};
