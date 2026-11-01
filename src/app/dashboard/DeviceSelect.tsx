import { useEffect, useState } from "react";
import { Select, SelectContent, SelectItem, SelectTrigger, SelectValue } from "@/components/ui/select";
import { native } from "@/core/native";
import type { AudioDevices } from "@/core/types";

let cache: AudioDevices | null = null;

export function useAudioDevices() {
  const [devices, setDevices] = useState<AudioDevices | null>(cache);
  const [error, setError] = useState<string | null>(null);
  useEffect(() => {
    native
      .audioListDevices()
      .then((d) => {
        cache = d;
        setDevices(d);
      })
      .catch((e) => setError((e as { message?: string }).message ?? "Couldn't list audio devices"));
  }, []);
  return { devices, error };
}

const DEFAULT = "__default__";

export function DeviceSelect({
  kind,
  value,
  onChange,
  id,
}: {
  kind: "input" | "output";
  value: string | null;
  onChange: (id: string | null) => void;
  id?: string;
}) {
  const { devices } = useAudioDevices();
  const list = kind === "input" ? devices?.inputs ?? [] : devices?.outputs ?? [];
  const def = kind === "input" ? list.find((d) => d.isDefaultComms) ?? list.find((d) => d.isDefault) : list.find((d) => d.isDefault);
  return (
    <Select value={value ?? DEFAULT} onValueChange={(v) => onChange(v === DEFAULT ? null : v)}>
      <SelectTrigger id={id} className="h-10 w-[340px] text-[13.5px]">
        <SelectValue />
      </SelectTrigger>
      <SelectContent>
        <SelectItem value={DEFAULT}>
          Default{def ? <span className="text-muted-foreground"> · {def.name}</span> : null}
        </SelectItem>
        {list.map((d) => (
          <SelectItem key={d.id} value={d.id}>
            {d.name}
          </SelectItem>
        ))}
      </SelectContent>
    </Select>
  );
}
