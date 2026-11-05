// Inline shortcut recorder (port of the old ShortcutRecorder.tsx): captures
// a key combo, validates it locally and natively, and warns about conflicts.

import { useEffect, useRef, useState } from "react";
import { Kbd } from "@/components/common";
import { Button } from "@/components/ui/button";
import { SHORTCUT_LABELS } from "@/core/defaults";
import { native } from "@/core/native";
import { acceleratorFromEvent, duplicateActions, knownConflict, validateAccelerator } from "@/core/shortcuts";
import type { AppSettings, ShortcutAction } from "@/core/types";
import { OS_NAME } from "@/core/platform";

export function ShortcutRecorder({
  action,
  shortcuts,
  onSave,
  onCancel,
}: {
  action: ShortcutAction;
  shortcuts: AppSettings["shortcuts"];
  onSave: (key: string) => void;
  onCancel: () => void;
}) {
  const [key, setKey] = useState<string | null>(null);
  const [error, setError] = useState<string | null>(null);
  const [saving, setSaving] = useState(false);
  const box = useRef<HTMLDivElement>(null);

  useEffect(() => {
    box.current?.focus();
  }, []);

  const onKeyDown = (e: React.KeyboardEvent) => {
    e.preventDefault();
    e.stopPropagation();
    if (e.key === "Escape" && !e.ctrlKey && !e.altKey && !e.shiftKey) {
      onCancel();
      return;
    }
    const acc = acceleratorFromEvent(e.nativeEvent);
    if (!acc) return; // only modifiers so far
    const v = validateAccelerator(acc);
    setKey(acc);
    setError(v.ok ? null : v.reason);
  };

  const dupes = key ? duplicateActions(shortcuts, key, action) : [];
  const taken = key ? knownConflict(key) : null;

  const save = async () => {
    if (!key || error) return;
    setSaving(true);
    try {
      const ok = await native.shortcutsValidate(key);
      if (!ok) {
        setError(`${OS_NAME} can't register this combination`);
        return;
      }
      onSave(key);
    } catch {
      setError("Couldn't validate this shortcut");
    } finally {
      setSaving(false);
    }
  };

  return (
    <div className="mt-2 flex flex-col gap-2">
      <div className="flex items-center gap-2">
        <div
          ref={box}
          tabIndex={0}
          role="textbox"
          aria-label={`Press the new shortcut for ${SHORTCUT_LABELS[action].label}`}
          onKeyDown={onKeyDown}
          className="flex h-10 min-w-[220px] flex-1 items-center rounded-lg border-2 border-ring/60 bg-muted/40 px-3 text-[13.5px] outline-none"
        >
          {key ? <Kbd keys={key} /> : <span className="text-muted-foreground">Press the new keys…</span>}
        </div>
        <Button size="sm" onClick={() => void save()} disabled={!key || !!error || saving || dupes.length > 0}>
          Save
        </Button>
        <Button size="sm" variant="ghost" onClick={onCancel}>
          Cancel
        </Button>
      </div>
      {error ? <p className="text-[13px] text-danger">{error}</p> : null}
      {!error && dupes.length ? (
        <p className="text-[13px] text-danger">
          Already used by “{dupes.map((d) => SHORTCUT_LABELS[d].label).join("”, “")}”. Pick other keys, or change that shortcut first.
        </p>
      ) : null}
      {!error && taken ? <p className="text-[13px] text-warn">Often taken by {taken}; it may not work while that app is open.</p> : null}
    </div>
  );
}
