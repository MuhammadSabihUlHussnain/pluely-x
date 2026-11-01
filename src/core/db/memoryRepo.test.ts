import { describe, expect, it } from "vitest";
import { sessionToMarkdown } from "./markdown";
import { createMemoryRepo } from "./memoryRepo";
import { toFtsQuery, transcriptRowId, type BriefRow } from "./types";

async function seeded() {
  const repo = createMemoryRepo();
  await repo.createSession({ id: "s1", mode: "interview", startedAt: 1000 });
  await repo.createSession({ id: "s2", mode: "meeting", title: "Q4 launch sync", startedAt: 5000 });
  await repo.addTranscript({ id: transcriptRowId("s1", "them-1-0"), sessionId: "s1", speaker: "them", text: "Tell me about Kafka consumers", t0Ms: 0, t1Ms: 900 });
  await repo.addTranscript({ id: transcriptRowId("s1", "me-1-1000"), sessionId: "s1", speaker: "me", text: "We used idempotent upserts", t0Ms: 1000, t1Ms: 2000 });
  await repo.addCard({
    id: "c1",
    sessionId: "s2",
    kind: "typed",
    question: "Draft the follow-up email",
    content: "Send it today.\n- recap",
    screenshotPath: null,
    status: "done",
    createdAt: 6000,
  });
  return repo;
}

describe("in-memory repository", () => {
  it("lists sessions newest first with card counts", async () => {
    const repo = await seeded();
    const list = await repo.listSessions();
    expect(list.map((s) => s.id)).toEqual(["s2", "s1"]);
    expect(list[0].cardCount).toBe(1);
    expect(list[1].cardCount).toBe(0);
  });

  it("searches transcripts, cards and titles (prefix words, all must match)", async () => {
    const repo = await seeded();
    expect((await repo.listSessions({ query: "kafka" })).map((s) => s.id)).toEqual(["s1"]);
    expect((await repo.listSessions({ query: "idempot" })).map((s) => s.id)).toEqual(["s1"]);
    expect((await repo.listSessions({ query: "follow-up email" })).map((s) => s.id)).toEqual(["s2"]);
    expect((await repo.listSessions({ query: "launch" })).map((s) => s.id)).toEqual(["s2"]);
    expect(await repo.listSessions({ query: "kafka launch" })).toEqual([]);
    expect((await repo.listSessions({ query: "kafka" }))[0].snippet).toBe("Tell me about Kafka consumers");
  });

  it("returns ordered detail and upserts transcript rows by id", async () => {
    const repo = await seeded();
    await repo.addTranscript({ id: transcriptRowId("s1", "them-1-0"), sessionId: "s1", speaker: "them", text: "Tell me about Kafka consumers, please", t0Ms: 0, t1Ms: 950 });
    const d = await repo.getSessionDetail("s1");
    expect(d?.transcript.map((t) => t.text)).toEqual(["Tell me about Kafka consumers, please", "We used idempotent upserts"]);
    await repo.deleteTranscript(transcriptRowId("s1", "me-1-1000"));
    expect((await repo.getSessionDetail("s1"))?.transcript).toHaveLength(1);
  });

  it("updates, ends and deletes sessions with their children", async () => {
    const repo = await seeded();
    await repo.updateSession("s2", { endedAt: 9000, summaryJson: '{"mode":"meeting"}', title: undefined });
    expect(await repo.getSession("s2")).toMatchObject({ endedAt: 9000, title: "Q4 launch sync" });
    await repo.upsertNotes({ sessionId: "s2", kind: "meeting", contentJson: "{}", updatedAt: 1 });
    await repo.upsertNotes({ sessionId: "s2", kind: "meeting", contentJson: '{"decisions":[]}', updatedAt: 2 });
    expect((await repo.getSessionDetail("s2"))?.notes).toEqual([{ sessionId: "s2", kind: "meeting", contentJson: '{"decisions":[]}', updatedAt: 2 }]);
    await repo.deleteSession("s2");
    expect(await repo.getSessionDetail("s2")).toBeNull();
    expect(repo.data.cards).toHaveLength(0);
    expect(repo.data.notes).toHaveLength(0);
    await repo.deleteAll();
    expect(await repo.listSessions()).toEqual([]);
  });

  it("keeps one default brief per mode", async () => {
    const repo = createMemoryRepo();
    const brief = (id: string): BriefRow => ({ id, name: id, mode: null, content: "x", filesJson: "[]", createdAt: 1, updatedAt: 1 });
    await repo.saveBrief(brief("a"));
    await repo.saveBrief(brief("b"));
    await repo.setDefaultBrief("interview", "a");
    expect((await repo.defaultBrief("interview"))?.id).toBe("a");
    await repo.setDefaultBrief("interview", "b");
    expect((await repo.defaultBrief("interview"))?.id).toBe("b");
    expect((await repo.listBriefs()).find((x) => x.id === "a")?.mode).toBeNull();
    await repo.setDefaultBrief("interview", null);
    expect(await repo.defaultBrief("interview")).toBeNull();
  });

  it("stores and resets custom mode prompts", async () => {
    const repo = createMemoryRepo();
    await repo.setModePrompt("coding", "Be terse.");
    expect(await repo.getModePrompts()).toEqual({ coding: "Be terse." });
    await repo.resetModePrompt("coding");
    expect(await repo.getModePrompts()).toEqual({});
  });

  it("calls onChange after writes (browser persistence)", async () => {
    let n = 0;
    const repo = createMemoryRepo(undefined, () => n++);
    await repo.createSession({ id: "x", mode: "ask", startedAt: 1 });
    await repo.updateSession("x", { title: "t" });
    expect(n).toBe(2);
  });

  it("exports a session as Markdown", async () => {
    const repo = await seeded();
    const md = sessionToMarkdown((await repo.getSessionDetail("s1"))!);
    expect(md).toContain("## Transcript");
    expect(md).toContain("**Them** `0:00` Tell me about Kafka consumers");
    expect(md).toContain("**You** `0:01` We used idempotent upserts");
  });
});

describe("FTS query sanitising", () => {
  it("quotes words and adds prefix matching", () => {
    expect(toFtsQuery('kafka "con')).toBe('"kafka"* "con"*');
    expect(toFtsQuery("  ")).toBeNull();
    expect(toFtsQuery("C++ AND OR")).toBe('"C"* "AND"* "OR"*');
  });
});
