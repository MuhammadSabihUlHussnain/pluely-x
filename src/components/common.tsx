// Small shared building blocks: keyboard hints, icon buttons with real
// tooltips, copy button.

import { Check, Copy } from "lucide-react";
import * as React from "react";
import { Tooltip, TooltipContent, TooltipTrigger } from "@/components/ui/tooltip";
import { displayParts } from "@/core/shortcuts";
import { cn } from "@/lib/utils";

export function Kbd({ keys, className }: { keys: string; className?: string }) {
  const parts = displayParts(keys);
  if (!parts.length) return null;
  return (
    <span className={cn("inline-flex items-center gap-0.5 align-middle", className)}>
      {parts.map((p, i) => (
        <kbd
          key={i}
          className="inline-flex h-[18px] min-w-[18px] items-center justify-center rounded-[5px] border border-border bg-muted/60 px-1 font-sans text-[11.5px] font-medium leading-none text-muted-foreground"
        >
          {p}
        </kbd>
      ))}
    </span>
  );
}

/** Tooltip with an optional shortcut hint. */
export function Hint({
  label,
  keys,
  side = "bottom",
  children,
}: {
  label: React.ReactNode;
  keys?: string;
  side?: "top" | "bottom" | "left" | "right";
  children: React.ReactElement;
}) {
  return (
    <Tooltip>
      <TooltipTrigger asChild>{children}</TooltipTrigger>
      <TooltipContent side={side}>
        <span className="flex items-center gap-2">
          <span>{label}</span>
          {keys ? <Kbd keys={keys} /> : null}
        </span>
      </TooltipContent>
    </Tooltip>
  );
}

type IconButtonProps = React.ButtonHTMLAttributes<HTMLButtonElement> & {
  label: string;
  keys?: string;
  side?: "top" | "bottom" | "left" | "right";
  active?: boolean;
  tone?: "default" | "danger" | "warn" | "ok";
  size?: "sm" | "md";
};

export const IconButton = React.forwardRef<HTMLButtonElement, IconButtonProps>(function IconButton(
  { label, keys, side, active, tone = "default", size = "md", className, children, ...rest },
  ref,
) {
  return (
    <Hint label={label} keys={keys} side={side}>
      <button
        ref={ref}
        type="button"
        aria-label={label}
        aria-pressed={active}
        className={cn(
          "inline-flex shrink-0 items-center justify-center rounded-lg text-muted-foreground transition-colors duration-150",
          "hover:bg-foreground/[0.08] hover:text-foreground active:bg-foreground/[0.12] disabled:pointer-events-none disabled:opacity-40",
          size === "md" ? "size-8 [&_svg]:size-[17px]" : "size-7 [&_svg]:size-[15px]",
          active && "bg-foreground/[0.1] text-foreground",
          tone === "danger" && "text-danger hover:text-danger",
          tone === "warn" && "text-warn hover:text-warn",
          tone === "ok" && "text-ok hover:text-ok",
          className,
        )}
        {...rest}
      >
        {children}
      </button>
    </Hint>
  );
});

export function useCopy(text: string) {
  const [copied, setCopied] = React.useState(false);
  const timer = React.useRef<ReturnType<typeof setTimeout> | null>(null);
  const copy = React.useCallback(() => {
    void navigator.clipboard
      .writeText(text)
      .then(() => {
        setCopied(true);
        if (timer.current) clearTimeout(timer.current);
        timer.current = setTimeout(() => setCopied(false), 1600);
      })
      .catch(() => {});
  }, [text]);
  return { copied, copy };
}

export function CopyButton({ text, label = "Copy answer", size = "sm" }: { text: string; label?: string; size?: "sm" | "md" }) {
  const { copied, copy } = useCopy(text);
  return (
    <IconButton label={copied ? "Copied" : label} onClick={copy} size={size} tone={copied ? "ok" : "default"}>
      {copied ? <Check /> : <Copy />}
    </IconButton>
  );
}

/** Renders `code`, **bold** and *italic* inside a one-line headline. */
export function InlineMd({ text }: { text: string }) {
  const parts = text.split(/(`[^`]+`|\*\*[^*]+\*\*|\*[^*\s][^*]*\*)/g).filter(Boolean);
  return (
    <>
      {parts.map((p, i) => {
        if (p.startsWith("`") && p.endsWith("`") && p.length > 2)
          return (
            <code key={i} className="rounded-[5px] bg-foreground/[0.09] px-[0.3em] py-[0.05em] font-mono text-[0.86em] font-medium">
              {p.slice(1, -1)}
            </code>
          );
        if (p.startsWith("**") && p.endsWith("**") && p.length > 4) return <strong key={i}>{p.slice(2, -2)}</strong>;
        if (p.startsWith("*") && p.endsWith("*") && p.length > 2) return <em key={i}>{p.slice(1, -1)}</em>;
        return <React.Fragment key={i}>{p}</React.Fragment>;
      })}
    </>
  );
}
