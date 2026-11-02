import { describe, expect, it } from "vitest";
import { runQuickScan, scanVerdict, type ScanDeps, type ScanStep } from "./quickScan";

function deps(over: Partial<ScanDeps> = {}): ScanDeps & { restarts: string[]; updates: number } {
  const d = {
    restarts: [] as string[],
    updates: 0,
    keysStatus: async () => ({ anthropic: true, deepgram: true }),
    testKey: async () => ({ ok: true }),
    devices: async () => ({ inputs: 1, outputs: 1 }),
    listening: () => true,
    modeAutoAnswers: () => ({ label: "Interview", autoAnswers: true }),
    restartSource: async (s: "me" | "them") => {
      d.restarts.push(s);
      return true;
    },
    clearStuckAnswers: () => 0,
    runAudioCheck: async () => ({ phase: "ok", how: "test_tone" }) as const,
    shortcutFailures: async () => 0,
    onUpdate: () => {
      d.updates++;
    },
    ...over,
  };
  return d;
}

const byId = (steps: ScanStep[], id: string) => steps.find((s) => s.id === id)!;

describe("runQuickScan", () => {
  it("passes everything on a healthy live session and restarts both captures, them first", async () => {
    const d = deps();
    const steps = await runQuickScan(d);
    expect(d.restarts).toEqual(["them", "me"]);
    expect(byId(steps, "capture").status).toBe("fixed");
    expect(steps.filter((s) => s.id !== "capture").every((s) => s.status === "ok")).toBe(true);
    expect(scanVerdict(steps)).toBe("ok");
    expect(d.updates).toBeGreaterThan(steps.length); // live progress, not one final dump
  });

  it("skips the key tests when a key is missing and points to Settings", async () => {
    let tested = 0;
    const steps = await runQuickScan(
      deps({
        keysStatus: async () => ({ anthropic: false, deepgram: true }),
        testKey: async () => {
          tested++;
          return { ok: true };
        },
      }),
    );
    expect(byId(steps, "keys")).toMatchObject({ status: "fail", action: "settings" });
    expect(byId(steps, "claude").status).toBe("skipped");
    expect(tested).toBe(1); // only Deepgram
  });

  it("flags a mode without auto-answers", async () => {
    const steps = await runQuickScan(deps({ modeAutoAnswers: () => ({ label: "Lecture", autoAnswers: false }) }));
    expect(byId(steps, "mode").status).toBe("warn");
    expect(scanVerdict(steps)).toBe("warn");
  });

  it("does not touch audio when not listening", async () => {
    const d = deps({ listening: () => false });
    const steps = await runQuickScan(d);
    expect(d.restarts).toEqual([]);
    for (const id of ["capture", "answers", "audio"]) expect(byId(steps, id).status).toBe("skipped");
  });

  it("reports silent computer audio as a failure", async () => {
    const steps = await runQuickScan(deps({ runAudioCheck: async () => ({ phase: "problem", kind: "silent" }) }));
    expect(byId(steps, "audio").status).toBe("fail");
    expect(scanVerdict(steps)).toBe("fail");
  });

  it("keeps going after a step throws", async () => {
    const steps = await runQuickScan(
      deps({
        devices: async () => {
          throw { message: "boom" };
        },
      }),
    );
    expect(byId(steps, "devices")).toMatchObject({ status: "fail", detail: "boom" });
    expect(byId(steps, "shortcuts").status).toBe("ok");
  });

  it("names the capture that wouldn't restart", async () => {
    const steps = await runQuickScan(deps({ restartSource: async (s) => s === "me" }));
    expect(byId(steps, "capture").status).toBe("fail");
    expect(byId(steps, "capture").detail).toContain("computer audio");
  });

  it("reports cleared stuck answers as fixed", async () => {
    const steps = await runQuickScan(deps({ clearStuckAnswers: () => 2 }));
    expect(byId(steps, "answers")).toMatchObject({ status: "fixed", detail: "Cleared 2 stuck answers." });
  });
});
