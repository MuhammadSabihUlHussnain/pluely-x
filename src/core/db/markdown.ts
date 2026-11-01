// Session → Markdown export (per session, or everything).

import { parseAnswer } from "../ai/answer";
import type { LectureNotes, MeetingNotes, SessionSummaryJson } from "../ai/schemas";
import { MODE_META } from "../defaults";
import { formatClock, formatDuration } from "../util";
import type { SessionDetail } from "./types";

export function parseSummary(json: string | null): SessionSummaryJson | null {
  if (!json) return null;
  try {
    return JSON.parse(json) as SessionSummaryJson;
  } catch {
    return null;
  }
}

export function sessionTitle(s: { title: string; mode: keyof typeof MODE_META; startedAt: number }): string {
  if (s.title.trim()) return s.title.trim();
  return `${MODE_META[s.mode].label} · ${new Date(s.startedAt).toLocaleDateString(undefined, { month: "short", day: "numeric" })}`;
}

export function summaryToMarkdown(sum: SessionSummaryJson): string {
  const out: string[] = [];
  switch (sum.mode) {
    case "meeting":
      out.push(sum.summary, "");
      if (sum.decisions.length) out.push("### Decisions", ...sum.decisions.map((d) => `- ${d}`), "");
      if (sum.action_items.length)
        out.push(
          "### Action items",
          ...sum.action_items.map((a) => `- [ ] ${a.task}${a.owner ? ` — ${a.owner}` : ""}${a.due ? ` (due ${a.due})` : ""}`),
          "",
        );
      out.push("### Follow-up email", `**Subject:** ${sum.follow_up_email.subject}`, "", sum.follow_up_email.body, "");
      break;
    case "interview":
      out.push(sum.overview, "", "### Questions asked");
      sum.questions.forEach((q, i) =>
        out.push(`${i + 1}. **${q.question}**`, `   - You said: ${q.your_answer}`, `   - Feedback: ${q.feedback}`),
      );
      out.push("", "### Tips for next time", ...sum.tips.map((t) => `- ${t}`), "");
      break;
    case "lecture":
      out.push(sum.study_notes, "");
      if (sum.key_terms.length) out.push("### Key terms", ...sum.key_terms.map((t) => `- **${t.term}** — ${t.definition}`), "");
      if (sum.practice_questions.length) {
        out.push("### Practice questions");
        sum.practice_questions.forEach((q, i) => out.push(`${i + 1}. ${q.question}`, `   - *Answer:* ${q.answer}`));
        out.push("");
      }
      break;
    case "coding":
      sum.problems.forEach((p) =>
        out.push(`### ${p.problem}`, "", `**Approach:** ${p.approach}`, "", `**Complexity:** ${p.complexity}`, "", p.solution, ""),
      );
      break;
    case "ask":
      out.push(sum.summary, "");
      break;
  }
  return out.join("\n");
}

export function notesToMarkdown(kind: string, json: string): string {
  try {
    if (kind === "meeting") {
      const n = JSON.parse(json) as MeetingNotes;
      return [
        ...(n.decisions.length ? ["**Decisions**", ...n.decisions.map((d) => `- ${d}`)] : []),
        ...(n.action_items.length
          ? ["**Action items**", ...n.action_items.map((a) => `- [ ] ${a.task}${a.owner ? ` — ${a.owner}` : ""}${a.due ? ` (due ${a.due})` : ""}`)]
          : []),
        ...(n.open_questions.length ? ["**Open questions**", ...n.open_questions.map((q) => `- ${q}`)] : []),
      ].join("\n");
    }
    if (kind === "lecture") {
      const n = JSON.parse(json) as LectureNotes;
      return [
        ...(n.key_points.length ? ["**Key points**", ...n.key_points.map((p) => `- ${p}`)] : []),
        ...(n.terms.length ? ["**Terms**", ...n.terms.map((t) => `- **${t.term}** — ${t.definition}`)] : []),
      ].join("\n");
    }
  } catch {
    /* fall through */
  }
  return "```json\n" + json + "\n```";
}

export function sessionToMarkdown(d: SessionDetail): string {
  const s = d.session;
  const sum = parseSummary(s.summaryJson);
  const title = sum?.title || sessionTitle(s);
  const duration = s.endedAt ? formatDuration(s.endedAt - s.startedAt) : "—";
  const lines: string[] = [
    `# ${title}`,
    "",
    `*${MODE_META[s.mode].label} · ${new Date(s.startedAt).toLocaleString()} · ${duration}*`,
    "",
  ];
  if (sum) lines.push("## Summary", "", summaryToMarkdown(sum), "");
  for (const n of d.notes) lines.push(`## Notes (${n.kind})`, "", notesToMarkdown(n.kind, n.contentJson), "");
  if (d.cards.length) {
    lines.push("## Answers", "");
    for (const c of d.cards) {
      const p = parseAnswer(c.content);
      lines.push(`### ${c.question || (c.screenshotPath ? "Screenshot" : "Question")}`, "");
      if (c.screenshotPath) lines.push(`*Screenshot: ${c.screenshotPath}*`, "");
      if (p.headline) lines.push(`**${p.headline}**`, "");
      if (p.body) lines.push(p.body, "");
      if (p.details) lines.push(p.details, "");
    }
  }
  if (d.transcript.length) {
    lines.push("## Transcript", "");
    for (const t of d.transcript) {
      lines.push(`**${t.speaker === "them" ? "Them" : "You"}** \`${formatClock(t.t0Ms)}\` ${t.text}  `);
    }
    lines.push("");
  }
  return lines.join("\n");
}

export function downloadText(filename: string, text: string): void {
  const blob = new Blob([text], { type: "text/markdown;charset=utf-8" });
  const url = URL.createObjectURL(blob);
  const a = document.createElement("a");
  a.href = url;
  a.download = filename;
  document.body.appendChild(a);
  a.click();
  a.remove();
  setTimeout(() => URL.revokeObjectURL(url), 1000);
}

export function safeFilename(name: string): string {
  return name.replace(/[\\/:*?"<>|]+/g, "-").replace(/\s+/g, " ").trim().slice(0, 80) || "session";
}
