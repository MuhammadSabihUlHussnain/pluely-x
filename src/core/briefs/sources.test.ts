import { describe, expect, it } from "vitest";
import { composeBrief, fairCaps, parseSources, serializeSources, sourceNames, type BriefDoc } from "./sources";

const doc = (name: string, chars: number, extra: Partial<BriefDoc> = {}): BriefDoc => ({
  id: name,
  name,
  size: chars,
  text: "x".repeat(chars),
  ...extra,
});

describe("fairCaps", () => {
  it("keeps everything when it fits", () => {
    expect(fairCaps([10, 20, 30], 100)).toEqual([10, 20, 30]);
  });
  it("keeps small parts whole and splits the rest evenly", () => {
    // 3 parts, 100 budget: the 10 fits its share (33); the other two split the remaining 90.
    expect(fairCaps([10, 500, 800], 100)).toEqual([10, 45, 45]);
  });
  it("never exceeds the budget", () => {
    const caps = fairCaps([400, 300, 2000, 5], 1000);
    expect(caps.reduce((a, b) => a + b, 0)).toBeLessThanOrEqual(1000);
    expect(caps[3]).toBe(5);
  });
});

describe("composeBrief", () => {
  it("joins notes and every document with headers, in order", () => {
    const c = composeBrief({ notes: "Meeting with Sam.", docs: [doc("resume.pdf", 5, { pages: 2 }), doc("jd.pdf", 3, { pages: 1 })] });
    expect(c.content).toBe("Meeting with Sam.\n\n=== resume.pdf (2 pages) ===\nxxxxx\n\n=== jd.pdf (1 page) ===\nxxx");
    expect(c.trimmed).toEqual({ notes: 0, docs: [0, 0] });
  });

  it("gives every document a share when over the limit instead of dropping the later ones", () => {
    const c = composeBrief({ notes: "", docs: [doc("big.pdf", 5000), doc("resume.pdf", 300), doc("jd.pdf", 4000)] }, 2000);
    expect(c.content.length).toBeLessThanOrEqual(2000);
    expect(c.content).toContain("=== resume.pdf ===\n" + "x".repeat(300) + "\n");
    expect(c.content).toContain("=== jd.pdf ===");
    expect(c.content).toContain("more characters not included");
    expect(c.trimmed.docs[1]).toBe(0);
    expect(c.trimmed.docs[0]).toBeGreaterThan(0);
    expect(c.trimmed.docs[2]).toBeGreaterThan(0);
  });

  it("is deterministic (byte-stable for prompt caching)", () => {
    const s = { notes: "n", docs: [doc("a.pdf", 9000), doc("b.pdf", 9000)] };
    expect(composeBrief(s, 5000).content).toBe(composeBrief(s, 5000).content);
  });
});

describe("stored format", () => {
  it("round-trips documents and notes", () => {
    const s = { notes: "hi", docs: [doc("a.pdf", 3, { pages: 1 })] };
    const row = { content: "ignored", filesJson: serializeSources(s) };
    expect(parseSources(row)).toEqual(s);
    expect(sourceNames(row)).toEqual(["a.pdf"]);
  });

  it("reads legacy briefs (merged text + file names) as editable notes", () => {
    const row = { content: "Old merged text", filesJson: JSON.stringify([{ name: "old.pdf", size: 10, chars: 15 }]) };
    expect(parseSources(row)).toEqual({ notes: "Old merged text", docs: [] });
    expect(sourceNames(row)).toEqual(["old.pdf"]);
    expect(parseSources({ content: "x", filesJson: "not json" })).toEqual({ notes: "x", docs: [] });
  });
});
