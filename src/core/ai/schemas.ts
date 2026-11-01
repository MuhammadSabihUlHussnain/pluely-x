// JSON schemas for structured outputs (output_config.format) and the matching
// TypeScript shapes. Every object sets additionalProperties:false and lists
// all of its properties as required, as structured outputs expect.

import type { Mode } from "../types";

type Schema = Record<string, unknown>;

const str: Schema = { type: "string" };
const strList: Schema = { type: "array", items: str };

function obj(properties: Record<string, Schema>): Schema {
  return {
    type: "object",
    properties,
    required: Object.keys(properties),
    additionalProperties: false,
  };
}
function list(item: Schema): Schema {
  return { type: "array", items: item };
}

const actionItem = obj({ owner: str, task: str, due: str });
const term = obj({ term: str, definition: str });

// ---- Rolling notes -------------------------------------------------------
export interface MeetingNotes {
  decisions: string[];
  action_items: { owner: string; task: string; due: string }[];
  open_questions: string[];
}
export interface LectureNotes {
  key_points: string[];
  terms: { term: string; definition: string }[];
}
export type RollingNotes = MeetingNotes | LectureNotes;

export const NOTES_SCHEMAS: Record<"meeting" | "lecture", Schema> = {
  meeting: obj({ decisions: strList, action_items: list(actionItem), open_questions: strList }),
  lecture: obj({ key_points: strList, terms: list(term) }),
};

export const EMPTY_NOTES: { meeting: MeetingNotes; lecture: LectureNotes } = {
  meeting: { decisions: [], action_items: [], open_questions: [] },
  lecture: { key_points: [], terms: [] },
};

// ---- Session summaries ---------------------------------------------------
export interface MeetingSummary {
  title: string;
  summary: string;
  decisions: string[];
  action_items: { owner: string; task: string; due: string }[];
  follow_up_email: { subject: string; body: string };
}
export interface InterviewSummary {
  title: string;
  overview: string;
  questions: { question: string; your_answer: string; feedback: string }[];
  tips: string[];
}
export interface LectureSummary {
  title: string;
  study_notes: string;
  key_terms: { term: string; definition: string }[];
  practice_questions: { question: string; answer: string }[];
}
export interface CodingSummary {
  title: string;
  problems: { problem: string; approach: string; solution: string; complexity: string }[];
}
export interface AskSummary {
  title: string;
  summary: string;
}
export type SessionSummaryJson =
  | ({ mode: "meeting" } & MeetingSummary)
  | ({ mode: "interview" } & InterviewSummary)
  | ({ mode: "lecture" } & LectureSummary)
  | ({ mode: "coding" } & CodingSummary)
  | ({ mode: "ask" } & AskSummary);

export const SUMMARY_SCHEMAS: Record<Mode, Schema> = {
  meeting: obj({
    title: str,
    summary: str,
    decisions: strList,
    action_items: list(actionItem),
    follow_up_email: obj({ subject: str, body: str }),
  }),
  interview: obj({
    title: str,
    overview: str,
    questions: list(obj({ question: str, your_answer: str, feedback: str })),
    tips: strList,
  }),
  lecture: obj({
    title: str,
    study_notes: str,
    key_terms: list(term),
    practice_questions: list(obj({ question: str, answer: str })),
  }),
  coding: obj({
    title: str,
    problems: list(obj({ problem: str, approach: str, solution: str, complexity: str })),
  }),
  ask: obj({ title: str, summary: str }),
};

/** Parses a structured-output stream; tolerates a stray code fence. */
export function parseJsonOutput<T>(text: string): T | null {
  const trimmed = text.trim().replace(/^```(?:json)?\s*/i, "").replace(/```\s*$/, "");
  try {
    return JSON.parse(trimmed) as T;
  } catch {
    return null;
  }
}
