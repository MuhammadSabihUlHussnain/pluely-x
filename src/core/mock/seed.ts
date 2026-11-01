// Demo library for the browser mock (fictional people and companies).

import type { MemoryData } from "../db/memoryRepo";
import { MOCK_NOTES, MOCK_SUMMARIES } from "./script";

const DAY = 86_400_000;

export function seedDemoData(now = Date.now()): MemoryData {
  const s1 = "ses_demo_interview";
  const s2 = "ses_demo_meeting";
  const s3 = "ses_demo_lecture";
  const s4 = "ses_demo_coding";
  const t1 = now - 1 * DAY - 3 * 3_600_000;
  const t2 = now - 2 * DAY - 5 * 3_600_000;
  const t3 = now - 4 * DAY - 1 * 3_600_000;
  const t4 = now - 6 * DAY - 2 * 3_600_000;
  const tr = (sessionId: string, i: number, speaker: "me" | "them", text: string, t0: number) => ({
    id: `${sessionId}:${speaker}-1-${t0}`,
    sessionId,
    speaker,
    text,
    t0Ms: t0,
    t1Ms: t0 + text.length * 55,
    _i: i,
  });

  return {
    sessions: [
      {
        id: s1,
        mode: "interview",
        title: "Platform engineer interview — Priya",
        startedAt: t1,
        endedAt: t1 + 31 * 60_000,
        briefId: "brief_resume",
        summaryJson: JSON.stringify({ mode: "interview", ...(MOCK_SUMMARIES.interview as object) }),
      },
      {
        id: s2,
        mode: "meeting",
        title: "Q4 launch plan sync",
        startedAt: t2,
        endedAt: t2 + 44 * 60_000,
        briefId: null,
        summaryJson: JSON.stringify({ mode: "meeting", ...(MOCK_SUMMARIES.meeting as object) }),
      },
      {
        id: s3,
        mode: "lecture",
        title: "Dijkstra's algorithm",
        startedAt: t3,
        endedAt: t3 + 52 * 60_000,
        briefId: null,
        summaryJson: JSON.stringify({ mode: "lecture", ...(MOCK_SUMMARIES.lecture as object) }),
      },
      {
        id: s4,
        mode: "coding",
        title: "Two Sum (hash map and two pointers)",
        startedAt: t4,
        endedAt: t4 + 18 * 60_000,
        briefId: null,
        summaryJson: JSON.stringify({ mode: "coding", ...(MOCK_SUMMARIES.coding as object) }),
      },
    ],
    transcript: [
      tr(s1, 0, "them", "Thanks for joining today. I'm Priya, I lead the platform team here.", 2_000),
      tr(s1, 1, "them", "So, to start, can you walk me through a project you're most proud of?", 7_500),
      tr(s1, 2, "me", "Sure. The one I'd pick is the live analytics pipeline at Northwind.", 13_000),
      tr(s1, 3, "me", "Reports were fifteen minutes stale, and we got that down to under two seconds.", 17_200),
      tr(s1, 4, "them", "Tell me about a time you disagreed with a teammate. How did you handle it?", 62_000),
      tr(s1, 5, "me", "Yeah, so there was a time a teammate wanted to rewrite our auth service right before a launch.", 68_000),
      tr(s2, 0, "them", "Okay, let's get started. The goal today is to lock the Q4 launch plan.", 1_500),
      tr(s2, 1, "them", "We decided the pricing page moves to next sprint, so marketing has more time.", 6_000),
      tr(s2, 2, "them", "Alex, can you give us a quick status on the onboarding flow?", 11_000),
      tr(s2, 3, "me", "Sure. Onboarding is in QA and on track for the twenty-eighth.", 15_000),
      tr(s3, 0, "them", "Today we're covering Dijkstra's algorithm, which finds shortest paths from one source.", 1_000),
      tr(s3, 1, "them", "The key idea is greedy. We always settle the closest unvisited vertex.", 8_000),
      tr(s4, 0, "them", "Given an array of integers and a target, return the indices of the two numbers that add up to the target.", 1_000),
      tr(s4, 1, "them", "How would you approach it, and what's the complexity?", 9_000),
      tr(s4, 2, "me", "I'd use a hash map from value to index and check the complement in a single pass.", 13_000),
    ].map(({ _i, ...row }) => row),
    cards: [
      {
        id: "card_demo_1",
        sessionId: s1,
        kind: "auto",
        question: "So, to start, can you walk me through a project you're most proud of?",
        content:
          "I'd pick the live analytics pipeline I built at Northwind — it cut dashboard latency from 15 minutes to under two seconds.\n- **Situation:** reports refreshed every 15 minutes.\n- **Action:** moved ingestion to Kafka with a streaming aggregator.\n- **Result:** p95 freshness 1.8 s; \"wrong number\" tickets down 70%.",
        screenshotPath: null,
        status: "done",
        createdAt: t1 + 9_000,
      },
      {
        id: "card_demo_2",
        sessionId: s2,
        kind: "auto",
        question: "Alex, can you give us a quick status on the onboarding flow?",
        content:
          "Onboarding is in QA now and on track for the October 28 release.\n- Two minor QA bugs, both fixed today.\n- Beta activation went from 41% to 58%.",
        screenshotPath: null,
        status: "done",
        createdAt: t2 + 12_000,
      },
      {
        id: "card_demo_3",
        sessionId: s4,
        kind: "screen",
        question: "",
        content:
          "Use a hash map from value to index: one pass, returning as soon as `target - x` has been seen.\n- **Time:** O(n)\n- **Space:** O(n)\n\nDetails\n```python\nclass Solution:\n    def twoSum(self, nums, target):\n        seen = {}\n        for i, x in enumerate(nums):\n            if target - x in seen:\n                return [seen[target - x], i]\n            seen[x] = i\n```",
        screenshotPath: `mock/screenshots/${s4}/shot_demo.jpg`,
        status: "done",
        createdAt: t4 + 4_000,
      },
    ],
    notes: [
      { sessionId: s2, kind: "meeting", contentJson: JSON.stringify(MOCK_NOTES.meeting), updatedAt: t2 + 30 * 60_000 },
      { sessionId: s3, kind: "lecture", contentJson: JSON.stringify(MOCK_NOTES.lecture), updatedAt: t3 + 40 * 60_000 },
    ],
    briefs: [
      {
        id: "brief_resume",
        name: "Résumé + platform engineer JD",
        mode: "interview",
        content:
          "RÉSUMÉ — Alex Rivera\nSenior Software Engineer, Northwind Labs (2022–present)\n- Led the live analytics pipeline: Kafka, Flink, Postgres; p95 freshness from 15 min to 1.8 s\n- Mentored 3 engineers; owned on-call for the data platform\nSoftware Engineer, Contoso (2019–2022)\n- Built the order service with an outbox pattern on Postgres\nSkills: Go, Python, TypeScript, Kafka, Kubernetes, PostgreSQL, AWS\n\nJOB DESCRIPTION — Platform Engineer, Fabrikam\n- Own the deploy pipeline and internal developer platform\n- Experience with distributed systems and consistency trade-offs\n- Kubernetes, Terraform, Go",
        filesJson: JSON.stringify([{ name: "alex-rivera-resume.pdf", size: 84_211, chars: 1_904 }]),
        createdAt: now - 9 * DAY,
        updatedAt: now - 2 * DAY,
      },
      {
        id: "brief_agenda",
        name: "Q4 launch agenda",
        mode: null,
        content: "Agenda: onboarding status, pricing page timing, beta scope, launch email owner.",
        filesJson: "[]",
        createdAt: now - 3 * DAY,
        updatedAt: now - 3 * DAY,
      },
    ],
    modePrompts: [],
  };
}
