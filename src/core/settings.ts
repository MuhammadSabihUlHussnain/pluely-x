// Settings store. Rust owns AppSettings (settings.json); this mirrors it and
// stays in sync across windows through the broadcast `settings://changed`.
// API keys live in Windows Credential Manager and are only ever reported as
// { present, hint } by keys_status.

import { create } from "zustand";
import { DEFAULT_SETTINGS } from "./defaults";
import { native } from "./native";
import type { AppSettings, DeepPartial, KeysStatus, Mode, ShortcutAction, ShortcutFailure } from "./types";
import { deepMerge } from "./util";

interface SettingsState {
  settings: AppSettings;
  loaded: boolean;
  error: string | null;
  load(): Promise<void>;
  update(patch: DeepPartial<AppSettings>): Promise<void>;
  setMode(mode: Mode): Promise<void>;
}

export const useSettings = create<SettingsState>((set, get) => ({
  settings: DEFAULT_SETTINGS,
  loaded: false,
  error: null,
  async load() {
    try {
      const s = await native.settingsGet();
      set({ settings: deepMerge(DEFAULT_SETTINGS, s), loaded: true, error: null });
    } catch (e) {
      set({ loaded: true, error: (e as { message?: string }).message ?? "Couldn't load settings" });
    }
  },
  async update(patch) {
    // Optimistic: apply locally, then take Rust's validated copy.
    const prev = get().settings;
    set({ settings: deepMerge(prev, patch) });
    try {
      const s = await native.settingsUpdate(patch);
      set({ settings: deepMerge(DEFAULT_SETTINGS, s), error: null });
    } catch (e) {
      set({ settings: prev, error: (e as { message?: string }).message ?? "Couldn't save settings" });
      throw e;
    }
  },
  async setMode(mode) {
    if (get().settings.mode === mode) return;
    await get().update({ mode });
  },
}));

let subscribed = false;
/**
 * Loads settings, key status and shortcut status, and follows
 * `settings://changed`, `keys://changed` and `shortcut://error` from any window.
 */
export async function initSettings(): Promise<void> {
  if (!subscribed) {
    subscribed = true;
    await native.on("settings://changed", (s) => {
      useSettings.setState({ settings: deepMerge(DEFAULT_SETTINGS, s), loaded: true });
    });
    await native.on("keys://changed", (status) => useKeys.setState({ status }));
    await native.on("shortcut://error", (e) => {
      const h = useShortcutHealth.getState();
      const merged = [...h.failures.filter((f) => !e.failures.some((n) => n.action === f.action)), ...e.failures];
      h.setFailures(merged);
      void h.refresh();
    });
  }
  await Promise.all([useSettings.getState().load(), useKeys.getState().refresh(), useShortcutHealth.getState().refresh()]);
}

interface KeysState {
  status: KeysStatus | null;
  refresh(): Promise<KeysStatus | null>;
}

export const useKeys = create<KeysState>((set) => ({
  status: null,
  async refresh() {
    try {
      const status = await native.keysStatus();
      set({ status });
      return status;
    } catch {
      return null;
    }
  },
}));

interface ShortcutHealthState {
  failures: ShortcutFailure[];
  refresh(): Promise<void>;
  setFailures(f: ShortcutFailure[]): void;
  failed(action: ShortcutAction): ShortcutFailure | undefined;
}

/** Shortcuts Windows refused to register (often taken by another app). */
export const useShortcutHealth = create<ShortcutHealthState>((set, get) => ({
  failures: [],
  async refresh() {
    try {
      const st = await native.shortcutsStatus();
      set({ failures: st.failures });
    } catch {
      /* older native build: rely on shortcut://error */
    }
  },
  setFailures(failures) {
    set({ failures });
  },
  failed(action) {
    return get().failures.find((f) => f.action === action);
  },
}));
