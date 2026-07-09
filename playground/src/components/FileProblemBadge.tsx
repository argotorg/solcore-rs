import { CircleX, TriangleAlert } from "lucide-react";
import type { CompileResult, Diag } from "../compiler/types";

export type FileProblemSeverity = "error" | "warning";

export interface FileProblemSummary {
  severity: FileProblemSeverity;
  errorCount: number;
  warningCount: number;
  label: string;
}

function diagnosticFile(diagnostic: Diag): string | null {
  return diagnostic.primary?.file ?? diagnostic.labels[0]?.range.file ?? null;
}

function pluralize(count: number, singular: string): string {
  return `${count} ${singular}${count === 1 ? "" : "s"}`;
}

function summaryLabel(errorCount: number, warningCount: number): string {
  const parts: string[] = [];

  if (errorCount > 0) {
    parts.push(pluralize(errorCount, "error"));
  }

  if (warningCount > 0) {
    parts.push(pluralize(warningCount, "warning"));
  }

  return parts.join(", ");
}

export function fileProblemSummaries(
  result: CompileResult | null,
): Map<string, FileProblemSummary> {
  const counts = new Map<string, { errorCount: number; warningCount: number }>();

  for (const diagnostic of result?.diagnostics ?? []) {
    if (diagnostic.severity !== "error" && diagnostic.severity !== "warning") {
      continue;
    }

    const file = diagnosticFile(diagnostic);
    if (!file) {
      continue;
    }

    const current = counts.get(file) ?? { errorCount: 0, warningCount: 0 };
    if (diagnostic.severity === "error") {
      current.errorCount += 1;
    } else {
      current.warningCount += 1;
    }
    counts.set(file, current);
  }

  const summaries = new Map<string, FileProblemSummary>();

  for (const [file, { errorCount, warningCount }] of counts) {
    summaries.set(file, {
      severity: errorCount > 0 ? "error" : "warning",
      errorCount,
      warningCount,
      label: summaryLabel(errorCount, warningCount),
    });
  }

  return summaries;
}

interface FileProblemBadgeProps {
  summary: FileProblemSummary;
}

export function FileProblemBadge({ summary }: FileProblemBadgeProps): JSX.Element {
  const Icon = summary.severity === "error" ? CircleX : TriangleAlert;

  return (
    <span
      className={`file-problem-badge file-problem-badge--${summary.severity}`}
      role="img"
      aria-label={summary.label}
      title={summary.label}
    >
      <Icon size={12} strokeWidth={2.4} aria-hidden="true" />
    </span>
  );
}
