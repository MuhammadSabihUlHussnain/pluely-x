// Shortcut strings use the contract's Windows accelerator form, e.g.
// "ctrl+alt+l", "ctrl+backslash", "ctrl+alt+arrowup". Ported from the old
// lib/storage/shortcuts.storage.ts validation and display logic.

import type { AppSettings, ShortcutAction } from "./types";
import { IS_MAC } from "./platform";

const MODIFIERS = ["ctrl", "alt", "shift", "super"] as const;

const KEY_NAMES: Record<string, string> = {
  backslash: "\\",
  slash: "/",
  comma: ",",
  period: ".",
  minus: "-",
  equal: "=",
  semicolon: ";",
  quote: "'",
  backquote: "`",
  bracketleft: "[",
  bracketright: "]",
  arrowup: "↑",
  arrowdown: "↓",
  arrowleft: "←",
  arrowright: "→",
  enter: "Enter",
  space: "Space",
  tab: "Tab",
  escape: "Esc",
  backspace: "Backspace",
  delete: "Delete",
  insert: "Insert",
  home: "Home",
  end: "End",
  pageup: "PgUp",
  pagedown: "PgDn",
};

const VALID_KEYS = new Set<string>([
  ..."abcdefghijklmnopqrstuvwxyz0123456789".split(""),
  ...Array.from({ length: 24 }, (_, i) => `f${i + 1}`),
  ...Object.keys(KEY_NAMES),
]);

/** Maps KeyboardEvent.code to the accelerator key name (layout-independent). */
export function keyFromCode(code: string): string | null {
  if (/^Key[A-Z]$/.test(code)) return code.slice(3).toLowerCase();
  if (/^Digit[0-9]$/.test(code)) return code.slice(5);
  if (/^Numpad[0-9]$/.test(code)) return code.slice(6);
  if (/^F([1-9]|1[0-9]|2[0-4])$/.test(code)) return code.toLowerCase();
  const map: Record<string, string> = {
    Backslash: "backslash",
    IntlBackslash: "backslash",
    Slash: "slash",
    Comma: "comma",
    Period: "period",
    Minus: "minus",
    Equal: "equal",
    Semicolon: "semicolon",
    Quote: "quote",
    Backquote: "backquote",
    BracketLeft: "bracketleft",
    BracketRight: "bracketright",
    ArrowUp: "arrowup",
    ArrowDown: "arrowdown",
    ArrowLeft: "arrowleft",
    ArrowRight: "arrowright",
    Enter: "enter",
    NumpadEnter: "enter",
    Space: "space",
    Tab: "tab",
    Escape: "escape",
    Backspace: "backspace",
    Delete: "delete",
    Insert: "insert",
    Home: "home",
    End: "end",
    PageUp: "pageup",
    PageDown: "pagedown",
  };
  return map[code] ?? null;
}

export interface KeyLike {
  ctrlKey: boolean;
  altKey: boolean;
  shiftKey: boolean;
  metaKey: boolean;
  code: string;
}

/** Accelerator for a key event, or null while only modifiers are held. */
export function acceleratorFromEvent(e: KeyLike): string | null {
  const key = keyFromCode(e.code);
  if (!key) return null;
  const parts: string[] = [];
  if (e.ctrlKey) parts.push("ctrl");
  if (e.altKey) parts.push("alt");
  if (e.shiftKey) parts.push("shift");
  if (e.metaKey) parts.push("super");
  parts.push(key);
  return parts.join("+");
}

export function normalizeAccelerator(key: string): string {
  const parts = key
    .toLowerCase()
    .split("+")
    .map((p) => p.trim())
    .filter(Boolean)
    .map((p) => (p === "control" || p === "cmdorctrl" || p === "cmd" ? "ctrl" : p === "meta" || p === "win" ? "super" : p))
    .map((p) => (p === "return" ? "enter" : p === "esc" ? "escape" : p))
    .map((p) => (p === "up" || p === "down" || p === "left" || p === "right" ? `arrow${p}` : p));
  const mods = MODIFIERS.filter((m) => parts.includes(m));
  const rest = parts.filter((p) => !(MODIFIERS as readonly string[]).includes(p));
  return [...mods, ...rest].join("+");
}

/** Needs at least one modifier and exactly one known key. */
export function validateAccelerator(key: string): { ok: true } | { ok: false; reason: string } {
  const parts = normalizeAccelerator(key).split("+");
  const mods = parts.filter((p) => (MODIFIERS as readonly string[]).includes(p));
  const keys = parts.filter((p) => !(MODIFIERS as readonly string[]).includes(p));
  if (keys.length !== 1) return { ok: false, reason: "Press one key together with a modifier" };
  if (!VALID_KEYS.has(keys[0])) return { ok: false, reason: `“${keys[0]}” can't be used in a shortcut` };
  if (!mods.length && !/^f\d+$/.test(keys[0])) return { ok: false, reason: "Add Ctrl, Alt or Shift" };
  if (mods.length === 1 && mods[0] === "shift") return { ok: false, reason: "Shift alone would block typing" };
  return { ok: true };
}

/** Display parts for <Kbd>, e.g. ["Ctrl", "Alt", "L"] (on a Mac ["⌃", "⌥", "L"]). */
export function displayParts(key: string, mac: boolean = IS_MAC): string[] {
  if (!key) return [];
  return normalizeAccelerator(key)
    .split("+")
    .map((p) => {
      if (p === "ctrl") return mac ? "⌃" : "Ctrl";
      if (p === "alt") return mac ? "⌥" : "Alt";
      if (p === "shift") return mac ? "⇧" : "Shift";
      if (p === "super") return mac ? "⌘" : "Win";
      if (KEY_NAMES[p]) return KEY_NAMES[p];
      if (/^f\d+$/.test(p)) return p.toUpperCase();
      return p.toUpperCase();
    });
}

export function displayAccelerator(key: string, mac: boolean = IS_MAC): string {
  return displayParts(key, mac).join(mac ? "" : "+");
}

/** Combos that Windows, Teams, Zoom or Office commonly own. */
const KNOWN_TAKEN: Record<string, string> = {
  "ctrl+shift+m": "Teams mute",
  "ctrl+shift+o": "Teams camera",
  "alt+a": "Zoom mute",
  "alt+v": "Zoom video",
  "ctrl+shift+escape": "Task Manager",
  "ctrl+alt+delete": "Windows security",
  "super+l": "Lock screen",
  "ctrl+c": "Copy",
  "ctrl+v": "Paste",
  "ctrl+x": "Cut",
  "ctrl+z": "Undo",
  "ctrl+s": "Save",
  "ctrl+f": "Find",
};

export function knownConflict(key: string): string | null {
  return KNOWN_TAKEN[normalizeAccelerator(key)] ?? null;
}

/** Other actions already bound (and enabled) to the same accelerator. */
export function duplicateActions(
  shortcuts: AppSettings["shortcuts"],
  key: string,
  except?: ShortcutAction,
): ShortcutAction[] {
  const k = normalizeAccelerator(key);
  return (Object.keys(shortcuts) as ShortcutAction[]).filter(
    (a) => a !== except && shortcuts[a].enabled && normalizeAccelerator(shortcuts[a].key) === k,
  );
}
