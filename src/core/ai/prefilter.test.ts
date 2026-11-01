import { describe, expect, it } from "vitest";
import { isCandidateQuestion, prefilter } from "./prefilter";

describe("question pre-filter", () => {
  it.each([
    "Can you walk me through a project you're most proud of?",
    "so tell me about a time you disagreed with a teammate",
    "Walk me through your resume",
    "How would you design a rate limiter",
    "What's your experience with Kafka",
    "why did you leave your last job",
    "Great. So how do you handle on-call",
    "Could you explain the trade-offs",
    "Describe a time you failed.",
    "Do you have any questions for us",
    "And you?",
    "What is a closure in JavaScript",
  ])("passes %s", (text) => {
    expect(isCandidateQuestion(text)).toBe(true);
  });

  it.each([
    "Thanks for joining today.",
    "We decided the pricing page moves to next sprint.",
    "Okay, let's get started.",
    "I know what you mean.",
    "That sounds great, I like that you measured the latency.",
    "Priya will own the launch email, due Friday.",
    "",
    "Right",
  ])("rejects %s", (text) => {
    expect(isCandidateQuestion(text)).toBe(false);
  });

  it("trusts the native question hint", () => {
    expect(prefilter("mm", true)).toEqual({ candidate: true, reason: "hint" });
  });

  it("is generous with wh-led statements so Claude can decide (∅)", () => {
    expect(isCandidateQuestion("How we run things here is a little different.")).toBe(true);
  });

  it("catches a question mark anywhere", () => {
    expect(prefilter("Right? Anyway, moving on").reason).toBe("question-mark");
  });
});
