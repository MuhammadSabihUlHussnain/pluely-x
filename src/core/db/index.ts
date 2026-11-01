// Picks the repository: SQLite inside Tauri, in-memory (persisted to
// localStorage and seeded with demo data) in a plain browser.

import { seedDemoData } from "../mock/seed";
import { isTauri } from "../native";
import { createMemoryRepo, type MemoryData } from "./memoryRepo";
import { createSqlRepo } from "./sqlRepo";
import type { Repo } from "./types";

const LS_DB = "pluelyx.mock.db";

function browserRepo(): Repo {
  let initial: MemoryData | null = null;
  try {
    const raw = localStorage.getItem(LS_DB);
    if (raw) initial = JSON.parse(raw) as MemoryData;
  } catch {
    /* ignore */
  }
  return createMemoryRepo(initial ?? seedDemoData(), (d) => {
    try {
      localStorage.setItem(LS_DB, JSON.stringify(d));
    } catch {
      /* quota or private mode: keep in memory */
    }
  });
}

let repo: Repo | null = null;

export function getRepo(): Repo {
  return (repo ??= isTauri ? createSqlRepo() : browserRepo());
}

export * from "./types";
