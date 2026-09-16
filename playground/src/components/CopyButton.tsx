import { Check, Copy, CircleX } from "lucide-react";
import { useEffect, useState } from "react";

async function copyText(text: string): Promise<void> {
  try {
    if (navigator.clipboard) {
      await navigator.clipboard.writeText(text);
      return;
    }
  } catch {
    // The selection-based fallback also works on HTTP development hosts.
  }
  const active = document.activeElement;
  const field = document.createElement("textarea");
  field.value = text;
  field.readOnly = true;
  field.tabIndex = -1;
  field.style.cssText = "position:fixed;opacity:0;pointer-events:none";
  field.setAttribute("aria-hidden", "true");
  document.body.append(field);
  try {
    field.focus({ preventScroll: true });
    field.select();
    if (!document.execCommand("copy")) throw new Error("Copy failed");
  } finally {
    field.remove();
    if (active instanceof HTMLElement) active.focus({ preventScroll: true });
  }
}

export function CopyButton({ text, label, showLabel = false }: {
  text: string; label: string; showLabel?: boolean;
}): JSX.Element {
  const [status, setStatus] = useState<"idle" | "copied" | "failed">("idle");
  useEffect(() => {
    if (status === "idle") return;
    const timer = window.setTimeout(() => setStatus("idle"), 2000);
    return () => window.clearTimeout(timer);
  }, [status]);
  useEffect(() => setStatus("idle"), [text]);
  const title = status === "copied" ? "Copied" : status === "failed" ? "Copy failed; select the text to copy it" : label;
  return <button type="button" className="button button--ghost problem-copy" aria-label={title} title={title}
    onClick={() => { void copyText(text).then(() => setStatus("copied"), () => setStatus("failed")); }}>
    {status === "copied" ? <Check size={14} aria-hidden="true" /> : status === "failed" ? <CircleX size={14} aria-hidden="true" /> : <Copy size={14} aria-hidden="true" />}
    {showLabel ? (status === "failed" ? "Copy failed" : title) : null}
    <span className="sr-only" role="status">{status === "idle" ? "" : title}</span>
  </button>;
}
