// Typed wrappers over the Rust commands. The UI talks to the backend only through here.
import { invoke } from "@tauri-apps/api/core";

export type Vec3 = [number, number, number];

export interface Slot { hex: string; triangles: number; nearest: [number, number] | null }
export interface Repair { weldedVertices: number; degenerateTriangles: number; duplicateTriangles: number; loosePieces: number; looseTriangles: number }
export interface Material { name: string; kind: "flat" | "image" | "vertex"; detail: string; faces: number }
export interface Estimate { layers: number; multicolorLayers: number; swaps: number }
export interface Settings { heightMm: number; keepRatio: number; flattenMm: number; repair: boolean; minPieceMm: number; orient: boolean; fillHoles: boolean; splitVertices: boolean; dropFloating: boolean; deepClean: boolean; rotation: number[][]; borderDetail: number; minIslandMm2: number; printer: string; nozzleMm: number; layerMm: number }

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
}

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
  parts: { name: string; triangles: number }[];
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
  repair: Repair;
  estimate: Estimate;
  settings: Settings;
  size: Size;
  waste: Waste;
  plate: Plate;
  canUndo: boolean;
  undoLabel: "stroke" | "merge" | "bucket" | "fix" | null;
  paintedTriangles: number;
}

export interface Hit { point: Vec3; normal: Vec3 }
/** What the bucket would fill: the colour found (or added), its slot now, and the triangles it reaches. */
export interface BucketPick { targetHex: string; added: boolean; slot: number | null; sampleHex: string; deltaE: number; tris: number[] }
export interface PaintResult extends Hit { tris: number[] }

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
  orientForPrint: (overhangDeg: number) => invoke<Summary>("orient_for_print", { overhangDeg }),
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
  setSize: (mm: Vec3) => invoke<Summary>("set_size", { mm }),
  undoSize: () => invoke<Summary>("undo_size"),
  resetSize: () => invoke<Summary>("reset_size"),
  fitToPlate: () => invoke<Summary>("fit_to_plate"),
  setOffset: (mm: [number, number]) => invoke<Summary>("set_offset", { mm }),
  /** Turns the model about X (0), Y (1) or Z (2); it lands back on the plate. */
  rotate: (axis: number, degrees: number) => invoke<TurnResult>("rotate", { axis, degrees }),
  resetRotation: () => invoke<TurnResult>("reset_rotation"),
  /** Cuts `t` of the way along `axis`, keeping the low or high side, and the other as its own object if asked. */
  cutModel: (axis: number, t: number, keepLow: boolean, keepOther: boolean) => invoke<Summary>("cut_model", { axis, t, keepLow, keepOther }),
  undoCut: () => invoke<Summary>("undo_cut"),
  reducePalette: (target: number) => invoke<Summary>("reduce_palette", { target }),
  mergeSlots: (from: number, into: number) => invoke<Summary>("merge_slots", { from, into }),
  setMeshOps: (keepRatio: number, flattenMm: number, repair: boolean, minPieceMm: number) =>
    invoke<Summary>("set_mesh_ops", { keepRatio, flattenMm, repair, minPieceMm }),
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
  paint: (origin: Vec3, dir: Vec3, radiusMm: number, slot: number) => invoke<PaintResult | null>("paint", { origin, dir, radiusMm, slot }),
  bucketPick: (origin: Vec3, dir: Vec3, region: boolean, add: boolean) => invoke<BucketPick | null>("bucket_pick", { origin, dir, region, add }),
  bucket: (origin: Vec3, dir: Vec3, slot: number, region: boolean, add: boolean) => invoke<Summary>("bucket", { origin, dir, slot, region, add }),
  endStroke: () => invoke<Summary>("end_stroke"),
  undo: () => invoke<Summary>("undo"),
  /** `objects` picks the printed objects that go in; null for all. Pieces are named in `lang`. `thumbnails`: large and small PNG of the plate. */
  export3mf: (path: string, lang: string, objects: number[] | null = null, thumbnails: [Uint8Array, Uint8Array] | null = null) =>
    invoke<void>("export_3mf", { path, lang, objects, thumbnails: thumbnails && thumbnails.map((b) => Array.from(b)) }),
  exportColorZip: (path: string, thicknessMm: number, mainSolid: boolean, lang: string) =>
    invoke<number>("export_color_zip", { path, thicknessMm, mainSolid, lang }),
};

/** Mesh operations a printability fix changes; omitted ones stay as they are. */
export interface MeshFix {
  repair?: boolean; orient?: boolean; fillHoles?: boolean; splitVertices?: boolean; dropFloating?: boolean; deepClean?: boolean;
  minPieceMm?: number; flattenMm?: number;
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
}
