import { BriefcaseBusiness, Code2, GraduationCap, ScanSearch, Users, type LucideIcon } from "lucide-react";
import type { Mode } from "@/core/types";

export const MODE_ICON: Record<Mode, LucideIcon> = {
  meeting: Users,
  interview: BriefcaseBusiness,
  coding: Code2,
  lecture: GraduationCap,
  ask: ScanSearch,
};

export function ModeIcon({ mode, className }: { mode: Mode; className?: string }) {
  const Icon = MODE_ICON[mode];
  return <Icon className={className} aria-hidden />;
}
