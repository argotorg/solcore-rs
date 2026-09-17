import { copyText } from "../share/copyText";
import { Check, Copy, CircleX } from "lucide-react";
import { useEffect, useState } from "react";

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
