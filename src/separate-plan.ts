import type { Vec3 } from "./api";

/** A clear, distinct colour for each new object: hues a golden angle apart. */
export function objectColour(group: number): Vec3 {
  const h = (group * 137.508 + 160) % 360, s = 0.62, l = 0.56;
  const k = (n: number) => (n + h / 30) % 12;
  const a = s * Math.min(l, 1 - l);
  const f = (n: number) => l - a * Math.max(-1, Math.min(k(n) - 3, 9 - k(n), 1));
  return [f(0), f(8), f(4)].map((v) => Math.round(v * 255)) as Vec3;
}

/** New object ids renumbered 0, 1, 2… in the order their first piece appears. */
export function compact(groups: (number | null)[]): (number | null)[] {
  const seen = new Map<number, number>();
  return groups.map((g) => {
    if (g === null) return null;
    if (!seen.has(g)) seen.set(g, seen.size);
    return seen.get(g)!;
  });
}
