// Typed wrappers over the Rust commands. The UI talks to the backend only through here.
import { invoke } from "@tauri-apps/api/core";

export type Vec3 = [number, number, number];

export interface Slot { hex: string; triangles: number; nearest: [number, number] | null }
export interface Repair { weldedVertices: number; degenerateTriangles: number; duplicateTriangles: number; loosePieces: number; looseTriangles: number }
export interface Material { name: string; kind: "flat" | "image" | "vertex"; detail: string; faces: number }
export interface Estimate { layers: number; multicolorLayers: number; swaps: number; purgeGrams: number }
export interface Settings { heightMm: number; keepRatio: number; flattenMm: number; repair: boolean; minPieceMm: number; borderDetail: number; minIslandMm2: number; printer: string; nozzleMm: number; layerMm: number }

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

export interface Summary {
  fileName: string;
  fileSize: number;
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
  canUndo: boolean;
  undoLabel: string | null;
  paintedTriangles: number;
}

export interface Hit { point: Vec3; normal: Vec3 }
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
  blenderStatus: () => invoke<string | null>("blender_status"),
  initialFile: () => invoke<string | null>("initial_file"),
  setBlenderPath: (path: string) => invoke<string>("set_blender_path", { path }),
  openBlend: (path: string) => invoke<Summary>("open_blend", { path }),
  setPalette: (colors: string[]) => invoke<Summary>("set_palette", { colors }),
  suggestPalette: (count: number) => invoke<Summary>("suggest_palette", { count }),
  setHeight: (mm: number) => invoke<Summary>("set_height", { mm }),
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
  previewMesh: async () => decodePreview(await invoke<ArrayBuffer>("preview_mesh")),
  pick: (origin: Vec3, dir: Vec3) => invoke<Hit | null>("pick", { origin, dir }),
  paint: (origin: Vec3, dir: Vec3, radiusMm: number, slot: number) => invoke<PaintResult | null>("paint", { origin, dir, radiusMm, slot }),
  endStroke: () => invoke<Summary>("end_stroke"),
  undo: () => invoke<Summary>("undo"),
  export3mf: (path: string) => invoke<void>("export_3mf", { path }),
  exportColorZip: (path: string, thicknessMm: number, mainSolid: boolean) => invoke<number>("export_color_zip", { path, thicknessMm, mainSolid }),
};
