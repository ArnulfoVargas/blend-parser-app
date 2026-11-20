// Typed wrappers over the Rust commands. The UI talks to the backend only through here.
import { invoke } from "@tauri-apps/api/core";

export type Vec3 = [number, number, number];

/** A spool colour: Bambu Lab's catalogue or the user's own. */
export interface Filament { id: string; brand: string; kind: string; nameEs: string; nameEn: string; hex: string; custom: boolean }
export interface Filaments { filaments: Filament[]; owned: string[]; inventoryOnly: boolean }
/** `hex` is what the slot prints (its filament's colour, when it has one); `detectedHex`, the colour found in the model. */
export interface Slot { hex: string; detectedHex: string; filament: Filament | null; filamentDeltaE: number | null; triangles: number; nearest: [number, number] | null }
export interface Repair {
  weldedVertices: number; degenerateTriangles: number; duplicateTriangles: number; loosePieces: number; looseTriangles: number;
  filledHoles: number; flippedTriangles: number; splitVertices: number; droppedPieces: number; finTriangles: number;
  hiddenPieces: number; hiddenTriangles: number;
}
/** What cloning made: how many copies went to the object's plate and to a new one, and how many did not fit. */
export interface Cloned { summary: Summary; plate: number; onPlate: number; newPlate: number | null; onNewPlate: number; dropped: number }
/** What each advanced repair finds on a mesh (see `bp_core::MeshIssues`). */
export interface Issues { holes: number; openEdges: number; flippedTriangles: number; sharedVertices: number; floatingPieces: number; strayTriangles: number; hiddenPieces: number }
export interface Material { name: string; kind: "flat" | "image" | "vertex"; detail: string; faces: number }
export interface Estimate { layers: number; multicolorLayers: number; swaps: number }
export interface Settings { heightMm: number; keepRatio: number; flattenMm: number; repair: boolean; minPieceMm: number; orient: boolean; fillHoles: boolean; splitVertices: boolean; dropFloating: boolean; deepClean: boolean; protectColors: boolean; protectDetail: boolean; subdivide: number; smooth: number; creaseDeg: number; hollowMm: number; drains: number; drainMm: number; thickenMm: number; removeHidden: boolean; rotation: number[][]; borderDetail: number; minIslandMm2: number; printer: string; nozzleMm: number; layerMm: number }

export interface PrinterProfile {
  id: string; // blank for a new custom printer
  vendor: string; // brand; empty for the user's own
  name: string;
  volumeMm: Vec3; // width, depth, height
  nozzles: number[];
  base: string | null; // Bambu Lab printer whose presets the 3MF names
  custom: boolean;
}
export interface Printers { profiles: PrinterProfile[]; selected: string; nozzleMm: number; nozzles: number[] }

export interface WastePrefs {
  layerMm: number | null; // null follows the printer's process
  filamentDiameterMm: number;
  densityGCm3: number;
  flushMultiplier: number;
  startPurgeMm3: number;
  primeMm3: number;
  flushReuse: number; // 0 to 0.9
}
export interface PlatePrefs { tower: boolean; towerDepthMm: number | null; towerGapMm: number; marginMm: number }
export interface Prefs { waste: WastePrefs; plate: PlatePrefs; wasteDefaults: WastePrefs; plateDefaults: PlatePrefs }

export interface Size {
  originalMm: Vec3;
  targetMm: Vec3; // before flattening
  axisScale: Vec3;
  previewScale: number; // preview units to mm
  stretch: Vec3; // per-axis stretch already in the preview positions
  canUndo: boolean;
}

export interface Waste {
  swaps: number; // in the order that flushes least
  flushMm3: number;
  reusedMm3: number;
  startMm3: number;
  towerMm3: number;
  totalMm3: number;
  totalG: number;
  totalM: number;
  towerHeightMm: number;
  prefs: WastePrefs;
  layerMm: number;
  nozzleMm: number;
  purgeChute: boolean;
  flushMm3Matrix: number[]; // from i to j at i * n + j
  flushSet: boolean[];
}

export interface Rect { x0: number; y0: number; x1: number; y1: number }
export interface Plate {
  area: Rect;
  excluded: Rect[];
  model: Rect;
  tower: Rect | null;
  usable: Rect;
  offPlate: boolean;
  inMargin: boolean;
  onExcluded: boolean;
  onTower: boolean;
  towerOffPlate: boolean;
  fitFactor: number;
  fits: boolean;
  prefs: PlatePrefs;
  needsTower: boolean;
  towerMm: [number, number];
  towerEstimated: boolean;
  offsetMm: [number, number]; // model centre from the plate centre
  rotated: boolean; // turned from the file's orientation
  canUndoCut: boolean;
  objects: number; // the model and the pieces cut off it that were kept
  objectIds: number[]; // their ids: 0 is the model, piece `id` is named with `id + 1`
  /** Split across plates, each plate's own check; empty on one plate. */
  plates: (PlateCheck & { objects: number; sizeMm: Vec3; centre: Vec3; fits: boolean; fitsHeight: boolean })[];
}
/** Where one plate's pieces land and what is wrong with them. */
export interface PlateCheck {
  area: Rect; excluded: Rect[]; model: Rect; tower: Rect | null; usable: Rect;
  offPlate: boolean; inMargin: boolean; onExcluded: boolean; onTower: boolean; towerOffPlate: boolean; fitFactor: number;
}

/** The turn with the least overhang: areas in mm² as now and after it; `down` faces the plate after it. */
export interface OrientationPlan { changes: boolean; rotation: number[][]; down: Vec3; overhangMm2: [number, number]; contactMm2: [number, number] }

/** A turn; `rigid` says how the preview moved when it could follow as it is: p' = matrix · p + shift. */
export interface TurnResult { summary: Summary; rigid: { matrix: number[][]; shift: Vec3 } | null }

/** What a failed command sends: a catalogue key and the values it names. */
export interface AppError { key: string; params: Record<string, string | number> }
export const isAppError = (e: unknown): e is AppError => typeof e === "object" && e !== null && "key" in e;

/** A step of opening a file. */
export interface Progress { key: string; name: string }

/** A file the model references (`.mtl`, glTF `.bin`, texture) and whether it was found. */
export interface Companion { kind: "materials" | "buffers" | "textures" | "unmapped"; name: string; found: boolean; required: boolean }

export interface Summary {
  fileName: string;
  fileSize: number;
  importWarnings: { code: string; detail: string }[];
  parts: { name: string; triangles: number; object: number; plate: number; previewStart: number; previewEnd: number }[];
  materials: Material[];
  slots: Slot[];
  sourceTriangles: number;
  triangles: number;
  splitTriangles: number;
  sizeMm: Vec3;
  bedMm: [number, number];
  fitsBed: boolean;
  maxHeightMm: number;
  fitsHeight: boolean;
  smallIslands: number;
  minDetailMm2: number;
  contactMm2: number;
  /** How far simplifying moved the surface at most, in mm; 0 unsimplified. */
  deviationMm: number;
  /** The model's volume, and what hollowing did (volumes in mm³; why not, if it did nothing). */
  volumeMm3: number;
  thicken: { movedVertices: number; maxOffsetMm: number; areaMm2: number } | null;
  hollow: { solidMm3: number; volumeMm3: number; cellMm: number; drains: number; drainsWanted: number; refused: "open" | "noRoom" | null } | null;
  repair: Repair;
  estimate: Estimate;
  settings: Settings;
  size: Size;
  waste: Waste;
  plate: Plate;
  canUndo: boolean;
  undoLabel: "stroke" | "merge" | "fix" | "cut" | "separate" | "connect" | "split" | "regionRemove" | "regionExtract" | "refine" | "bucket" | null;
  paintedTriangles: number;
  kit: KitOptions | null; // how the colour kit was last cut
}

/** How the kit is cut: joint detail, colour depth over a one-colour core, and the least piece kept apart. */
export interface KitOptions { voxelMm: number; depthMm: number; coreSlot: number | null; minPieceMm3: number; mix: boolean; contents: KitContents; pins?: KitPins | null }
/** Pins keying the kit's pieces together: their size and the gap round them. */
export interface KitPins { diameterMm: number; depthMm: number; clearanceMm: number }
/** What the kit ZIP holds besides its summary, and how sharp its pictures are. */
export interface KitContents { threeMf: boolean; stl: "none" | "piece" | "colour"; guide: boolean; text: boolean; quality: "low" | "medium" | "high" }
/** How far the kit export has got; 0 of 0 while it cannot tell. */
export interface KitProgress { stage: "split" | "plates" | "pictures" | "files"; done: number; total: number }
/** What the kit came out with: pieces, plates, and what to look at (too big, to review, left out). */
export interface KitResult { pieces: number; plates: number; oversize: number; review: number; dropped: number; pins: number; unpinned: number; bytes: number }
export interface Hit { point: Vec3; normal: Vec3 }
/** A wall: where it was measured, the point across, and its thickness in mm (null with nothing across). */
export interface Wall { point: Vec3; across: Vec3; mm: number | null }

/** What the bucket would fill: the colour found (or added), its slot now, and the triangles it reaches. */
export interface BucketPick { targetHex: string; added: boolean; slot: number | null; sampleHex: string; deltaE: number; tris: number[] }

/** The connectors of a cut: sizes in mm, centres on the plane in preview coordinates. */
export interface Connectors {
  kind: "pin" | "dowel"; pinsOnA: boolean; shape: "round" | "square" | "hexagon";
  sizeMm: number; depthMm: number; clearanceMm: number; angleDeg: number; at: Vec3[];
}
/** Per connector what is wrong with it (null when fine) and, when all are fine, the objects the cut leaves. */
export interface ConnectReview { issues: ("outside" | "overlap" | "thinWall" | "tooDeep" | null)[]; objects: number | null }

/** Cuts that make every piece fit the printer, and the room each plate has. */
export interface SplitProposal { cuts: [number[], number[], number[]]; roomMm: [number, number]; heightMm: number; sizeMm: Vec3 }
export interface SplitPlan {
  cuts: [number[], number[], number[]]; // preview coordinates
  layOnCut: boolean; gapMm: number;
  dowels: { shape: "round" | "square" | "hexagon"; sizeMm: number; depthMm: number; clearanceMm: number } | null;
}
export interface SplitPiece { name: string; sizeMm: Vec3; plate: number | null; laid: boolean; small: boolean; dowel: boolean }
export interface SplitReport { pieces: SplitPiece[]; plates: number; cuts: number; joints: number; jointsMissing: number; fits: boolean }

/** A disconnected piece of the model. */
export interface Piece { part: number; partName: string; object: number; triangles: number; sizeMm: Vec3; centre: Vec3; closed: boolean }
/** The pieces and, per preview triangle, the index of its piece (2³²−1 for none). */
export interface Pieces { pieces: Piece[]; triPiece: number[] }
/** A plane worth reviewing as a cut, in preview coordinates, with the signal behind it. */
export interface CutProposal {
  part: number; axis: 0 | 1 | 2; point: Vec3; normal: Vec3;
  signal: { kind: "neck"; areaMm2: number; beforeMm2: number; afterMm2: number } | { kind: "colour"; from: number; to: number };
}
export interface PaintResult extends Hit { tris: number[] }

/** The region selection: global preview triangle ids, and how many separate regions they make. */
export interface Selection { tris: number[]; count: number; regions: number }
export type SelectionOp = "grow" | "connected" | "smooth" | "invert" | "clear" | "get";
/** How a cut region is closed: a shell `mm` thick, the piece `clearanceMm` smaller than its pocket; or a cap `mm` deep (negative bulges out). */
export type Closure = { kind: "shell"; mm: number; clearanceMm?: number } | { kind: "cap"; mm: number };
/** The region goes, or each connected region becomes an object of its own (with `group`, all one object). */
export type RegionAction = { kind: "remove" } | { kind: "extract"; layFlat: boolean; group?: boolean };
/** What makes faces the same colour: the model's material, the filament slot, or the colour within ΔE. */
export type ColourMatch = { kind: "material" } | { kind: "filament" } | { kind: "similar"; deltaE: number };
/** What cutting the region out would make, in preview units: flat xyz triangles (9 numbers each). */
export interface RegionPreview { closure: number[]; pieces: number[][]; directions: Vec3[]; pieceMm3: number[]; restMm3: number; closed: boolean; small: number }

/** The paintable mesh, decoded from the binary preview payload. */
export interface PreviewMesh {
  positions: Float32Array; // 9 floats per leaf triangle
  offsets: Uint32Array; // leaves of triangle i: offsets[i]..offsets[i + 1]
  labels: Uint8Array; // filament slot per leaf
  sourceRgb: Uint8Array; // original colour per leaf
}

export function decodePreview(buf: ArrayBuffer): PreviewMesh {
  const head = new DataView(buf, 0, 8);
  const leaves = head.getUint32(0, true);
  const tris = head.getUint32(4, true);
  let at = 8;
  const positions = new Float32Array(buf, at, leaves * 9);
  at += leaves * 36;
  const offsets = new Uint32Array(buf, at, tris + 1);
  at += (tris + 1) * 4;
  const labels = new Uint8Array(buf, at, leaves);
  at += leaves;
  const sourceRgb = new Uint8Array(buf, at, leaves * 3);
  return { positions, offsets, labels, sourceRgb };
}

export const api = {
  analyzePrintability: (params: PrintabilityParams) => invoke<PrintabilityReport>("analyze_printability", { params }),
  fixMesh: (fix: MeshFix) => invoke<Summary>("fix_mesh", { fix }),
  /** Hollows to a wall of `wallMm` (0 fills it again) with drains through the base; one undo step. */
  hollowMesh: (wallMm: number, drains: number, drainMm: number) => invoke<Summary>("hollow_mesh", { wallMm, drains, drainMm }),
  /** Subdivides and smooths as one undo step; refused past `maxTriangles`. */
  smoothMesh: (subdivide: number, smooth: number, creaseDeg: number, maxTriangles: number) =>
    invoke<Summary>("smooth_mesh", { subdivide, smooth, creaseDeg, maxTriangles }),
  /** Built without the advanced repairs, and as the mesh is now. */
  meshIssues: () => invoke<{ before: Issues; after: Issues }>("mesh_issues"),
  orientForPrint: (overhangDeg: number) => invoke<Summary>("orient_for_print", { overhangDeg }),
  /** The best turn and what it gains, without turning; `setRotation` applies it. */
  planOrientation: (overhangDeg: number) => invoke<OrientationPlan>("plan_orientation", { overhangDeg }),
  setRotation: (rotation: number[][]) => invoke<Summary>("set_rotation", { rotation }),
  blenderStatus: () => invoke<string | null>("blender_status"),
  initialFile: () => invoke<string | null>("initial_file"),
  setBlenderPath: (path: string) => invoke<string>("set_blender_path", { path }),
  openBlend: (path: string, added: { files: string[]; textures: [string, string][]; library: string | null; blend: string | null }) =>
    invoke<Summary>("open_blend", { path, extras: added.files, textures: added.textures, library: added.library, blend: added.blend }),
  /** Opens a saved `.bpproj` in place of the open document; Blender is not needed. */
  openProject: (path: string) => invoke<Summary>("open_project", { path }),
  saveProject: (path: string) => invoke<void>("save_project", { path }),
  inspectModel: (path: string, extras: string[], library: string | null = null) =>
    invoke<Companion[]>("inspect_model", { path, extras, library }),
  setPalette: (colors: string[]) => invoke<Summary>("set_palette", { colors }),
  suggestPalette: (count: number) => invoke<Summary>("suggest_palette", { count }),
  filaments: () => invoke<Filaments>("filaments"),
  setFilamentOwned: (id: string, owned: boolean) => invoke<Filaments>("set_filament_owned", { id, owned }),
  saveFilament: (filament: Filament) => invoke<Filaments>("save_filament", { filament }),
  deleteFilament: (id: string) => invoke<Filaments>("delete_filament", { id }),
  setInventoryOnly: (on: boolean) => invoke<Filaments>("set_inventory_only", { on }),
  /** Puts a filament on a slot; null takes it off. */
  setSlotFilament: (slot: number, id: string | null) => invoke<Summary>("set_slot_filament", { slot, id }),
  setSize: (mm: Vec3) => invoke<Summary>("set_size", { mm }),
  undoSize: () => invoke<Summary>("undo_size"),
  resetSize: () => invoke<Summary>("reset_size"),
  fitToPlate: () => invoke<Summary>("fit_to_plate"),
  setOffset: (mm: [number, number]) => invoke<Summary>("set_offset", { mm }),
  /** Turns the model about X (0), Y (1) or Z (2); it lands back on the plate. */
  rotate: (axis: number, degrees: number) => invoke<TurnResult>("rotate", { axis, degrees }),
  resetRotation: () => invoke<TurnResult>("reset_rotation"),
  /** The exact preview plane; its coordinates include stretch, before uniform scale. */
  cutModel: (point: Vec3, normal: Vec3, keepLow: boolean, keepOther: boolean, layOnCut: boolean, selected: number[]) =>
    invoke<Summary>("cut_model", { point, normal, keepLow, keepOther, layOnCut, selected }),
  previewCut: (point: Vec3, normal: Vec3, keepLow: boolean, keepOther: boolean, layOnCut: boolean, selected: number[]) =>
    invoke<number>("preview_cut", { point, normal, keepLow, keepOther, layOnCut, selected }),
  undoCut: () => invoke<Summary>("undo_cut"),
  detectPieces: (weldMm: number) => invoke<Pieces>("detect_pieces", { weldMm }),
  /** Same group: one object; null: the piece stays in the object it is in. */
  separatePieces: (weldMm: number, groups: (number | null)[], layFlat: boolean) => invoke<Summary>("separate_pieces", { weldMm, groups, layFlat }),
  proposeCuts: (selected: number[]) => invoke<CutProposal[]>("propose_cuts", { selected }),
  proposeSplit: (gapMm: number) => invoke<SplitProposal>("propose_split", { gapMm }),
  previewSplit: (plan: SplitPlan) => invoke<SplitReport>("preview_split", { plan }),
  splitForPrinter: (plan: SplitPlan, dowelLabel: string) => invoke<Summary>("split_for_printer", { plan, dowelLabel }),
  addPlate: () => invoke<Summary>("add_plate"),
  moveToPlate: (object: number, plate: number) => invoke<Summary>("move_to_plate", { object, plate }),
  cloneObject: (object: number, label: string, gapMm?: number) => invoke<Summary>("clone_object", { object, label, gapMm }),
  /** `count` copies: the plate is laid out again; what does not fit goes to one new plate, the rest are not made. */
  cloneCopies: (object: number, count: number, label: string, gapMm: number) => invoke<Cloned>("clone_copies", { object, count, label, gapMm }),
  arrangePlates: (gapMm: number) => invoke<Summary>("arrange_plates", { gapMm }),
  clearPlate: (plate: number) => invoke<Summary>("clear_plate", { plate }),
  suggestConnectors: (point: Vec3, normal: Vec3, selected: number[], connectors: Connectors) =>
    invoke<Vec3[]>("suggest_connectors", { point, normal, selected, connectors }),
  /** Checks every connector on a copy and, when all are fine, makes the cut there. */
  previewConnectedCut: (point: Vec3, normal: Vec3, layOnCut: boolean, selected: number[], connectors: Connectors) =>
    invoke<ConnectReview>("preview_connected_cut", { point, normal, layOnCut, selected, connectors }),
  cutConnected: (point: Vec3, normal: Vec3, layOnCut: boolean, selected: number[], connectors: Connectors, dowelLabel: string) =>
    invoke<Summary>("cut_connected", { point, normal, layOnCut, selected, connectors, dowelLabel }),
  /** Adds to the region selection, or with `add` false takes from it; never paints. */
  selectRegion: (origin: Vec3, dir: Vec3, radiusMm: number, add: boolean, through: boolean) =>
    invoke<PaintResult | null>("select_region", { origin, dir, radiusMm, add, through }),
  selectionOp: (op: SelectionOp) => invoke<Selection>("selection_op", { op }),
  previewRegion: (closure: Closure, action: RegionAction) => invoke<RegionPreview>("preview_region", { closure, action }),
  applyRegion: (closure: Closure, action: RegionAction) => invoke<Summary>("apply_region", { closure, action }),
  /** Selects (or with `add` false, deselects) the faces the same colour as preview triangle `seed`; with `base`, starting from that selection. */
  selectByColour: (seed: number, how: ColourMatch, contiguous: boolean, add: boolean, base: number[] | null = null) =>
    invoke<Selection>("select_by_colour", { seed, how, contiguous, add, base }),
  /** Makes colour borders that cross triangles edges of the mesh; one undoable step. */
  refineToPaint: () => invoke<Summary>("refine_to_paint"),
  reducePalette: (target: number) => invoke<Summary>("reduce_palette", { target }),
  mergeSlots: (from: number, into: number) => invoke<Summary>("merge_slots", { from, into }),
  setMeshOps: (keepRatio: number, flattenMm: number, repair: boolean, minPieceMm: number, protectColors?: boolean, protectDetail?: boolean) =>
    invoke<Summary>("set_mesh_ops", { keepRatio, flattenMm, repair, minPieceMm, protectColors, protectDetail }),
  setColorOps: (borderDetail: number, minIslandMm2: number) => invoke<Summary>("set_color_ops", { borderDetail, minIslandMm2 }),
  printers: () => invoke<Printers>("printers"),
  /** Null while no file is open; the pick is remembered either way. */
  setPrinter: (id: string, nozzleMm: number) => invoke<Summary | null>("set_printer", { id, nozzleMm }),
  savePrinter: (profile: PrinterProfile, nozzleMm: number) => invoke<Printers>("save_printer", { profile, nozzleMm }),
  deletePrinter: (id: string) => invoke<Printers>("delete_printer", { id }),
  prefs: () => invoke<Prefs>("prefs"),
  /** Null while no file is open; the settings are saved either way. */
  setWaste: (waste: WastePrefs) => invoke<Summary | null>("set_waste", { waste }),
  setPlate: (plate: PlatePrefs) => invoke<Summary | null>("set_plate", { plate }),
  /** A typed flush for one swap; null goes back to the estimate. No slots clears them all. */
  setFlush: (from: number | null, to: number | null, mm3: number | null) => invoke<Summary>("set_flush", { from, to, mm3 }),
  previewMesh: async () => decodePreview(await invoke<ArrayBuffer>("preview_mesh")),
  pick: (origin: Vec3, dir: Vec3) => invoke<Hit | null>("pick", { origin, dir }),
  /** The wall where the ray lands, as the thickness check reads it; null off the model. */
  measureThickness: (origin: Vec3, dir: Vec3, toleranceMm: number) => invoke<Wall | null>("measure_thickness", { origin, dir, toleranceMm }),
  /** The mesh's edges, each once: positions (3 per vertex) and vertex pairs. */
  wireMesh: async () => {
    const buf = await invoke<ArrayBuffer>("wire_mesh");
    const head = new DataView(buf, 0, 8);
    const [vertices, edges] = [head.getUint32(0, true), head.getUint32(4, true)];
    return { positions: new Float32Array(buf, 8, vertices * 3), ends: new Uint32Array(buf, 8 + vertices * 12, edges * 2) };
  },
  paint: (origin: Vec3, dir: Vec3, radiusMm: number, slot: number) => invoke<PaintResult | null>("paint", { origin, dir, radiusMm, slot }),
  bucketPick: (origin: Vec3, dir: Vec3, region: boolean, add: boolean) => invoke<BucketPick | null>("bucket_pick", { origin, dir, region, add }),
  bucket: (origin: Vec3, dir: Vec3, slot: number, region: boolean, add: boolean) => invoke<Summary>("bucket", { origin, dir, slot, region, add }),
  endStroke: () => invoke<Summary>("end_stroke"),
  undo: () => invoke<Summary>("undo"),
  /** `objects` picks the printed objects that go in; null for all. Pieces are named in `lang`. `thumbnails`: large and small PNG of the plate. */
  export3mf: (path: string, lang: string, objects: number[] | null = null, thumbnails: [Uint8Array, Uint8Array] | null = null) =>
    invoke<void>("export_3mf", { path, lang, objects, thumbnails: thumbnails && thumbnails.map((b) => Array.from(b)) }),
  /** The colour kit: solid pieces on single-colour plates, STLs and an assembly guide, in one ZIP. */
  exportColorZip: (path: string, o: KitOptions, lang: string) => invoke<KitResult>("export_color_zip", { path, options: o, lang }),
  cancelColorZip: () => invoke<void>("cancel_color_zip"),
  exportColorShellsZip: (path: string, thicknessMm: number, mainSolid: boolean, lang: string) =>
    invoke<number>("export_color_shells_zip", { path, thicknessMm, mainSolid, lang }),
  exportColorAssembly: (path: string, thicknessMm: number, mainSolid: boolean) =>
    invoke<number>("export_color_assembly", { path, thicknessMm, mainSolid }),
};

/** Mesh operations a printability fix changes; omitted ones stay as they are. */
export interface MeshFix {
  repair?: boolean; orient?: boolean; fillHoles?: boolean; splitVertices?: boolean; dropFloating?: boolean; deepClean?: boolean;
  minPieceMm?: number; flattenMm?: number; thickenMm?: number; removeHidden?: boolean;
}
export interface PrintabilityParams {
  extrusionMm: number; perimeters: number; wallMm: number; detailMm: number;
  componentMm: number; overhangDeg: number; contactMm2: number; toleranceMm: number; samples: number;
}
export interface PrintabilityCheck {
  code: string; evidence: "checked" | "estimated" | "notEvaluated";
  severity: "pass" | "warning" | "error" | "incomplete"; count: number;
  measurement: number | null; threshold: number | null; locations: Vec3[];
}
export interface PrintabilityReport {
  params: PrintabilityParams; checks: PrintabilityCheck[]; triangles: number; sizeMm: Vec3;
  printer: string | null; nozzleMm: number | null; layerMm: number | null;
  thickness: { point: Vec3; mm: number | null }[];
  /** Flagged edges as their two ends, in mm. */
  edges: { open: [Vec3, Vec3][]; nonManifold: [Vec3, Vec3][]; winding: [Vec3, Vec3][] };
}
