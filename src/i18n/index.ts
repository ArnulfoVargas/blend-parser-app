// Interface language: the catalogues, the user's choice and the formats that go
// with it. Spanish is the reference catalogue and the fallback for missing keys; every other
// catalogue is typed against it, so `npm run build` fails when one misses or
// adds a key. To add a language see README, "Idiomas".
import { en } from "./en";
import { es, type Catalog, type Entry, type Key } from "./es";

export type { Key };

/** Languages offered in the selector, by their own name. */
export const LANGS = [
  { code: "es", name: "Español" },
  { code: "en", name: "English" },
] as const;
export type Lang = (typeof LANGS)[number]["code"];

const CATALOGS: Record<Lang, Catalog> = { es, en };
const FALLBACK: Lang = "en";
const STORAGE = "blendparser.lang";

export type Params = Record<string, string | number>;

/** The saved choice, else the first system language there is a catalogue for, else English. */
function initial(): Lang {
  try {
    const saved = localStorage.getItem(STORAGE);
    if (saved && saved in CATALOGS) return saved as Lang;
  } catch {
    // Storage blocked: fall through to the system language.
  }
  for (const tag of navigator.languages ?? [navigator.language]) {
    const base = tag.toLowerCase().split("-")[0];
    if (base in CATALOGS) return base as Lang;
  }
  return FALLBACK;
}

let lang: Lang = initial();
let plurals = new Intl.PluralRules(lang);
let numbers = numberLocale();

/** Numbers follow the system's region when it speaks the interface language
 *  (es-MX writes 1,234.5; es-ES 1.234,5), else the language's default. */
function numberLocale(): string {
  for (const tag of navigator.languages ?? [navigator.language]) {
    if (tag.toLowerCase().split("-")[0] === lang) return tag;
  }
  return lang;
}
const listeners: (() => void)[] = [];

export const currentLang = () => lang;

export function setLang(next: Lang) {
  if (!(next in CATALOGS) || next === lang) return;
  lang = next;
  plurals = new Intl.PluralRules(lang);
  numbers = numberLocale();
  try {
    localStorage.setItem(STORAGE, lang);
  } catch {
    // Not remembered, but the change still applies now.
  }
  document.documentElement.lang = lang;
  applyStatic();
  listeners.forEach((f) => f());
}

/** Runs `f` after every language change. */
export const onLangChange = (f: () => void) => listeners.push(f);

function pick(entry: Entry, count: number | undefined): string {
  if (typeof entry === "string") return entry;
  const form = count === undefined ? "other" : plurals.select(count);
  return (entry as Record<string, string>)[form] ?? entry.other;
}

function lookup(key: string): Entry | undefined {
  return (CATALOGS[lang] as Record<string, Entry>)[key] ?? (es as Record<string, Entry>)[key];
}

/** `{name}` placeholders filled from `params`; numbers come preformatted by the caller. */
function fill(text: string, params?: Params) {
  return params ? text.replace(/\{(\w+)\}/g, (m, k) => (k in params ? String(params[k]) : m)) : text;
}

/** The text for `key`; `params.count` picks the plural form. */
export function t(key: Key, params?: Params): string {
  const entry = lookup(key);
  return entry === undefined ? "" : fill(pick(entry, typeof params?.count === "number" ? params.count : undefined), params);
}

/** For keys built at run time (backend errors, progress steps): `fallback` when the key is unknown. */
export function tKey(key: string, params?: Params, fallback = ""): string {
  const entry = lookup(key);
  return entry === undefined ? fallback : fill(pick(entry, typeof params?.count === "number" ? params.count : undefined), params);
}

/** A number in the interface language, with `d` decimals at most (and at least, when `fixed`). */
export function num(n: number, d = 0, fixed = true): string {
  return n.toLocaleString(numbers, { maximumFractionDigits: d, minimumFractionDigits: fixed ? d : 0 });
}

/** Decimals that read well: two under 10, one under 100, none above. */
const readable = (n: number) => num(n, n < 10 ? 2 : n < 100 ? 1 : 0);

/** Grams, or kilograms past 1000 g. */
export const mass = (g: number) => (g < 1000 ? `${readable(g)} g` : `${readable(g / 1000)} kg`);
/** Metres, or kilometres past 1000 m. */
export const length = (m: number) => (m < 1000 ? `${readable(m)} m` : `${readable(m / 1000)} km`);
/** mm³, or cm³ past 10 000 mm³. */
export const volume = (mm3: number) => (mm3 < 10000 ? `${num(mm3)} mm³` : `${readable(mm3 / 1000)} cm³`);

/** A value for an `<input type="number">`, which always takes a dot: no grouping, at most `d` decimals. */
export const inputValue = (n: number, d = 2) => String(Math.round(n * 10 ** d) / 10 ** d);

/** A list joined as the language joins it: "0,2, 0,4 o 0,6". */
export const list = (items: string[]) => new Intl.ListFormat(lang, { type: "disjunction" }).format(items);

/** Fills every `[data-i18n]` text and every `[data-i18n-attr="attr:key;attr:key"]` attribute. */
export function applyStatic(root: ParentNode = document) {
  root.querySelectorAll<HTMLElement>("[data-i18n]").forEach((el) => {
    el.textContent = tKey(el.dataset.i18n!, undefined, el.textContent ?? "");
  });
  root.querySelectorAll<HTMLElement>("[data-i18n-attr]").forEach((el) => {
    for (const pair of el.dataset.i18nAttr!.split(";")) {
      const [attr, key] = pair.split(":");
      el.setAttribute(attr, tKey(key, undefined, el.getAttribute(attr) ?? ""));
    }
  });
}

document.documentElement.lang = lang;
