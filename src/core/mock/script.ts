// Scripted conversations and canned Claude answers for the browser mock.
// The people and companies here are fictional demo data.

import type { Mode, Source } from "../types";

export interface ScriptLine {
  source: Source;
  text: string;
  /** Silence before this line starts, ms. */
  pauseMs: number;
  /** Canned answer when Claude is asked about this line (∅ when absent). */
  answer?: string;
  /** Emit a mic "echo" of this Them line, later patched as echo. */
  echo?: boolean;
}

const A_PROJECT = `I'd pick the live analytics pipeline I built at Northwind — it cut dashboard latency from 15 minutes to under two seconds.
- **Situation:** reports refreshed every 15 minutes, so sales kept quoting stale numbers to customers.
- **Task:** I owned the redesign end to end with one other engineer, on a six-week deadline.
- **Action:** moved ingestion to Kafka with a streaming aggregator and added per-tenant backpressure.
- **Result:** p95 freshness went from 15 min to 1.8 s, and "wrong number" tickets dropped 70%.

Details
If they dig in, name the trade-off: you chose at-least-once delivery with idempotent upserts over exactly-once, which halved the operational complexity. A good closing line: *"The part I'm proudest of is that on-call pages went down, not up, after launch."*`;

const A_DISAGREE = `I disagreed with a teammate about rewriting our auth service, and we settled it with a three-day spike instead of a debate.
- **Situation:** he wanted a full rewrite two weeks before a launch; I thought targeted fixes were safer.
- **Task:** we had to decide within the sprint without stalling the rest of the team.
- **Action:** I proposed we each prototype for three days and compare on latency, risk and test coverage.
- **Result:** the numbers favoured incremental fixes, we shipped on time, and he led the rewrite the next quarter.

Details
Keep the tone generous: the point is that you turned a disagreement into an experiment and kept the relationship strong.`;

const A_DISTRIBUTED = `I've mostly built eventually consistent systems, and I choose the consistency level per path rather than per system.
- Payments and inventory used strongly consistent Postgres writes with row-level locks.
- Analytics and feeds ran on Kafka with idempotent consumers, so duplicate events were harmless.
- Under a partition we favoured availability for reads and consistency for writes.

Details
Concrete example: the order service wrote to Postgres and published events through an **outbox table** in the same transaction, so downstream consumers never saw an event for a rolled-back order.

\`\`\`sql
BEGIN;
INSERT INTO orders (id, total) VALUES ($1, $2);
INSERT INTO outbox (topic, payload) VALUES ('order.created', $3);
COMMIT;
\`\`\``;

const A_STATUS = `Onboarding is in QA now and on track for the October 28 release.
- The new three-step flow is code-complete; QA has two minor bugs open, both fixed today.
- Activation in the internal beta went from 41% to 58%.
- The only risk is the email verification provider — I'll confirm their rate limits by Thursday.`;

const A_BETA = `I'd support moving the beta up two weeks, as long as we cap it at 200 accounts.
- Onboarding and billing are ready; analytics export is the only feature still in progress.
- A capped beta gives us real usage data before the pricing page ships.
- I'd want a go/no-go check on the 14th based on the open bug count.`;

const A_TWO_SUM = `Use a hash map from value to index: one pass, checking whether \`target - x\` was already seen.
- **Time:** O(n) — each element is looked up and inserted once.
- **Space:** O(n) for the map.
- **Edge cases:** duplicates like \`[3, 3]\` with target 6, negative numbers, and no valid pair.

Details
\`\`\`python
def two_sum(nums: list[int], target: int) -> list[int]:
    seen: dict[int, int] = {}
    for i, x in enumerate(nums):
        if target - x in seen:
            return [seen[target - x], i]
        seen[x] = i
    return []
\`\`\`
Check the complement *before* inserting \`x\`, so an element is never paired with itself.`;

const A_SORTED = `If the array is sorted, use two pointers from both ends — O(n) time and O(1) space.
- Move the left pointer right when the sum is too small, the right pointer left when it's too large.
- It works because sorting makes the sum monotonic in each pointer.
- Mention that sorting an unsorted input first costs O(n log n) and loses the original indices.

Details
\`\`\`python
def two_sum_sorted(nums: list[int], target: int) -> list[int]:
    lo, hi = 0, len(nums) - 1
    while lo < hi:
        s = nums[lo] + nums[hi]
        if s == target:
            return [lo, hi]
        if s < target:
            lo += 1
        else:
            hi -= 1
    return []
\`\`\``;

export const SCRIPTS: Record<Exclude<Mode, "ask"> | "audioCheck", ScriptLine[]> = {
  interview: [
    { source: "them", pauseMs: 900, text: "Thanks for joining today. I'm Priya, I lead the platform team here." },
    {
      source: "them",
      pauseMs: 700,
      text: "So, to start, can you walk me through a project you're most proud of?",
      answer: A_PROJECT,
      echo: true,
    },
    {
      source: "me",
      pauseMs: 2600,
      text: "Sure. The one I'd pick is the live analytics pipeline at Northwind. Reports were fifteen minutes stale, and we got that down to under two seconds.",
    },
    {
      source: "them",
      pauseMs: 900,
      text: "How we run things here is a little different, the platform team owns the whole deploy pipeline.",
    },
    {
      source: "them",
      pauseMs: 1200,
      text: "Tell me about a time you disagreed with a teammate. How did you handle it?",
      answer: A_DISAGREE,
    },
    {
      source: "me",
      pauseMs: 2400,
      text: "Yeah, so there was a time a teammate wanted to rewrite our auth service right before a launch.",
    },
    {
      source: "them",
      pauseMs: 1500,
      text: "What's your experience with distributed systems, especially consistency trade-offs?",
      answer: A_DISTRIBUTED,
    },
  ],
  meeting: [
    { source: "them", pauseMs: 900, text: "Okay, let's get started. The goal today is to lock the Q4 launch plan." },
    { source: "them", pauseMs: 900, text: "We decided the pricing page moves to next sprint, so marketing has more time." },
    {
      source: "them",
      pauseMs: 900,
      text: "Alex, can you give us a quick status on the onboarding flow?",
      answer: A_STATUS,
      echo: true,
    },
    {
      source: "me",
      pauseMs: 2400,
      text: "Sure. Onboarding is in QA and on track for the twenty-eighth. Activation in the beta went from forty-one to fifty-eight percent.",
    },
    { source: "them", pauseMs: 900, text: "Great. Priya will own the launch email, due Friday." },
    {
      source: "them",
      pauseMs: 1100,
      text: "What do you think about moving the public beta up two weeks?",
      answer: A_BETA,
    },
  ],
  coding: [
    {
      source: "them",
      pauseMs: 900,
      text: "Here's the problem. Given an array of integers and a target, return the indices of the two numbers that add up to the target.",
    },
    {
      source: "them",
      pauseMs: 700,
      text: "How would you approach it, and what's the complexity?",
      answer: A_TWO_SUM,
    },
    {
      source: "me",
      pauseMs: 2600,
      text: "I'd use a hash map from value to index and check the complement in a single pass.",
    },
    {
      source: "them",
      pauseMs: 1100,
      text: "Nice. If the array were sorted, could you do it in constant space?",
      answer: A_SORTED,
    },
  ],
  lecture: [
    {
      source: "them",
      pauseMs: 900,
      text: "Today we're covering Dijkstra's algorithm, which finds shortest paths from one source in a graph with non-negative edge weights.",
    },
    {
      source: "them",
      pauseMs: 900,
      text: "The key idea is greedy. We always settle the unvisited vertex with the smallest tentative distance, using a priority queue.",
    },
    {
      source: "them",
      pauseMs: 900,
      text: "When we settle a vertex we relax its edges, meaning we check whether going through it gives a shorter path to each neighbour.",
    },
    {
      source: "them",
      pauseMs: 900,
      text: "With a binary heap the running time is O of E log V. Negative edges break it, and that's where Bellman-Ford comes in.",
    },
  ],
  audioCheck: [
    { source: "them", pauseMs: 700, text: "This is a sound check coming from your speakers." },
    { source: "me", pauseMs: 900, text: "Testing, one two three. Can you hear me?" },
  ],
};

/** Answers for typed questions in the mock, picked by keyword. */
export function cannedTypedAnswer(question: string): string {
  const q = question.toLowerCase();
  if (/star|behaviou?ral/.test(q)) {
    return `STAR keeps a behavioural answer short and concrete: Situation, Task, Action, Result.
- **Situation / Task:** one sentence each — just enough context.
- **Action:** most of your time; say "I", not "we", and name the specific steps you took.
- **Result:** end on a number or a clear outcome, then one line on what you learned.`;
  }
  if (/big.?o|complexity/.test(q)) {
    return `Big-O describes how cost grows with input size, ignoring constants.
- O(1) constant, O(log n) halving, O(n) one pass, O(n log n) good sorts, O(n²) nested loops.
- Always state time *and* space, and say which case (worst or average) you mean.`;
  }
  if (/email|follow.?up/.test(q)) {
    return `Send a short follow-up today that thanks them, restates the decision, and lists owners with dates.
- Subject: "Q4 launch plan — decisions and next steps".
- Three bullets: pricing page next sprint, beta capped at 200 accounts, launch email by Friday.
- Close with the date of the go/no-go check.`;
  }
  const topic = question.trim().replace(/[?.!]+$/, "");
  return `Short answer: ${topic ? `for “${topic.slice(0, 80)}”, ` : ""}start with the simplest option that you can verify quickly.
- Name the goal in one sentence so everyone agrees on what "done" means.
- Pick the approach with the fewest moving parts, and say why.
- Mention one risk and how you'd check it early.`;
}

export const SCREEN_ANSWER_GENERIC = `This is a LeetCode problem page: **1. Two Sum**, with a Python editor open on the right.
- The task: return the indices of two numbers in \`nums\` that add up to \`target\`.
- The editor already has the signature \`def twoSum(self, nums: List[int], target: int) -> List[int]\`.
- Constraints at the bottom say exactly one valid answer exists.`;

export const SCREEN_ANSWER_CODING = `Use a hash map from value to index: one pass, returning as soon as \`target - x\` has been seen.
- **Time:** O(n) — one lookup and one insert per element.
- **Space:** O(n) for the map.
- **Edge cases:** duplicates like \`[3, 3]\`, negative numbers; the constraints guarantee exactly one answer.

Details
\`\`\`python
class Solution:
    def twoSum(self, nums: List[int], target: int) -> List[int]:
        seen: dict[int, int] = {}
        for i, x in enumerate(nums):
            if target - x in seen:
                return [seen[target - x], i]
            seen[x] = i
        return []
\`\`\`
**Walkthrough** on \`nums = [2, 7, 11, 15], target = 9\`: at \`i = 1\`, \`9 - 7 = 2\` is already in \`seen\` at index 0, so return \`[0, 1]\`.`;

export const MOCK_NOTES = {
  meeting: {
    decisions: ["Pricing page moves to next sprint to give marketing more time"],
    action_items: [
      { owner: "Priya", task: "Write and send the launch email", due: "Friday" },
      { owner: "Alex", task: "Confirm the email provider's rate limits", due: "Thursday" },
    ],
    open_questions: ["Should the public beta move up two weeks?"],
  },
  lecture: {
    key_points: [
      "Dijkstra finds single-source shortest paths when all edge weights are non-negative",
      "Greedy: always settle the unvisited vertex with the smallest tentative distance",
      "Relaxing an edge = check whether going through a vertex shortens the path to a neighbour",
      "Binary-heap implementation runs in O(E log V)",
    ],
    terms: [
      { term: "Relaxation", definition: "Updating a neighbour's distance if a shorter path through the current vertex exists" },
      { term: "Priority queue", definition: "A structure that always returns the smallest key first; here, the closest vertex" },
      { term: "Bellman-Ford", definition: "A slower shortest-path algorithm that also handles negative edges" },
    ],
  },
};

export const MOCK_SUMMARIES: Record<Mode, unknown> = {
  meeting: {
    title: "Q4 launch plan sync",
    summary:
      "The team met to lock the Q4 launch plan. Onboarding is in QA and on track for October 28, with beta activation up from 41% to 58%. The pricing page moves to next sprint, and the team discussed moving the public beta up two weeks with a 200-account cap.",
    decisions: ["Pricing page moves to next sprint", "Public beta is capped at 200 accounts"],
    action_items: [
      { owner: "Priya", task: "Send the launch email", due: "Friday" },
      { owner: "Alex", task: "Confirm email provider rate limits", due: "Thursday" },
    ],
    follow_up_email: {
      subject: "Q4 launch plan — decisions and next steps",
      body: "Hi all,\n\nThanks for the time today. Quick recap:\n\n- Pricing page moves to next sprint.\n- Public beta: capped at 200 accounts, go/no-go on the 14th.\n- Priya owns the launch email (Friday); I'll confirm the email provider's rate limits by Thursday.\n\nBest,\nAlex",
    },
  },
  interview: {
    title: "Platform engineer interview — Priya",
    overview:
      "A 30-minute first-round interview focused on past projects, collaboration and distributed systems. Answers were concrete and well-structured.",
    questions: [
      {
        question: "Walk me through a project you're most proud of",
        your_answer: "The Northwind live analytics pipeline: 15-minute reports down to under two seconds.",
        feedback: "Strong numbers; add one sentence on your personal decisions.",
      },
      {
        question: "Tell me about a time you disagreed with a teammate",
        your_answer: "Auth-service rewrite settled with a three-day spike.",
        feedback: "Good STAR shape; finish with the result sooner.",
      },
    ],
    tips: ["Lead with the result, then the story", "Prepare one failure story", "Ask about on-call load at the end"],
  },
  lecture: {
    title: "Dijkstra's algorithm",
    study_notes:
      "## Dijkstra's algorithm\n- Single-source shortest paths, **non-negative** weights only\n- Greedy: settle the closest unvisited vertex first (priority queue)\n- Relax each outgoing edge of a settled vertex\n\n## Complexity\n- Binary heap: **O(E log V)**\n- Negative edges → use Bellman-Ford",
    key_terms: [
      { term: "Relaxation", definition: "Improving a tentative distance through a newly settled vertex" },
      { term: "Priority queue", definition: "Returns the vertex with the smallest tentative distance" },
    ],
    practice_questions: [
      { question: "Why does Dijkstra fail with negative edges?", answer: "A settled vertex could later get a shorter path, breaking the greedy invariant." },
      { question: "What is the running time with a binary heap?", answer: "O(E log V)." },
    ],
  },
  coding: {
    title: "Two Sum (hash map and two pointers)",
    problems: [
      {
        problem: "Two Sum",
        approach: "One-pass hash map of value → index, checking the complement before inserting.",
        solution: "```python\ndef two_sum(nums, target):\n    seen = {}\n    for i, x in enumerate(nums):\n        if target - x in seen:\n            return [seen[target - x], i]\n        seen[x] = i\n```",
        complexity: "O(n) time, O(n) space",
      },
    ],
  },
  ask: { title: "Screen questions", summary: "Asked about a LeetCode problem on screen and how to explain Big-O." },
};
