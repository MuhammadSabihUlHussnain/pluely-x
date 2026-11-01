import React from "react";
import { Streamdown } from "streamdown";
import "katex/dist/katex.min.css";
import { native } from "@/core/native";
import { cn } from "@/lib/utils";

interface MarkdownRendererProps {
  children: string;
  isStreaming?: boolean;
  className?: string;
}

/** Streamdown + shiki + KaTeX, styled by .answer-prose (typography plugin). */
export function Markdown({ children, isStreaming = false, className }: MarkdownRendererProps) {
  return (
    <div className={cn("prose answer-prose", className)}>
      <Streamdown
        isAnimating={isStreaming}
        shikiTheme={["github-light", "github-dark"]}
        components={COMPONENTS as never}
        controls={{ table: true, code: true, mermaid: false }}
      >
        {children}
      </Streamdown>
    </div>
  );
}

const COMPONENTS = {
  a: ({ children, href, ...props }: React.AnchorHTMLAttributes<HTMLAnchorElement>) => (
    <a
      href={href}
      className="text-you underline underline-offset-2 hover:opacity-80"
      onClick={(e) => {
        e.preventDefault();
        if (href) void native.openUrl(href).catch(() => {});
      }}
      {...props}
    >
      {children}
    </a>
  ),
};
