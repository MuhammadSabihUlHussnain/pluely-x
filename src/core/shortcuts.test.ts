import { describe, expect, it } from "vitest";
import { displayAccelerator, displayParts } from "./shortcuts";

describe("shortcut labels", () => {
  it("uses Ctrl/Alt words on Windows", () => {
    expect(displayParts("ctrl+alt+l", false)).toEqual(["Ctrl", "Alt", "L"]);
    expect(displayAccelerator("ctrl+alt+l", false)).toBe("Ctrl+Alt+L");
    expect(displayParts("super+shift+s", false)).toEqual(["Shift", "Win", "S"]);
  });

  it("uses the Mac modifier symbols on macOS", () => {
    expect(displayParts("ctrl+alt+l", true)).toEqual(["⌃", "⌥", "L"]);
    expect(displayAccelerator("ctrl+alt+l", true)).toBe("⌃⌥L");
    expect(displayParts("super+shift+s", true)).toEqual(["⇧", "⌘", "S"]); // the Mac order: ⌃⌥⇧⌘
  });
});
