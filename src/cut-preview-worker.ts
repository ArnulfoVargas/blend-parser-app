/// <reference lib="webworker" />
import { cutExtent } from "./cut-extent";
import type { Vec3 } from "./api";

let positions: Float32Array = new Float32Array();
self.onmessage = (event: MessageEvent<{ positions?: Float32Array; id?: number; ranges: { previewStart: number; previewEnd: number }[]; point: Vec3; normal: Vec3 }>) => {
  const request = event.data;
  if (request.positions) { positions = request.positions; return; }
  const extent = cutExtent(positions, request.ranges, request.point, request.normal);
  const pack = (box: typeof extent.low) => box.isEmpty() ? null : [box.min.toArray(), box.max.toArray()];
  self.postMessage({ id: request.id, low: pack(extent.low), high: pack(extent.high), boxes: extent.boxes.map(pack), segments: extent.segments }, { transfer: [extent.segments.buffer] });
};
