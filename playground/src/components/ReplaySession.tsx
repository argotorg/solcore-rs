import { lazy, Suspense } from "react";
import { Bug } from "lucide-react";
import { useWorkspaceStore } from "../store/workspace";

const ReplayCall = lazy(() => import("./ReplayCall"));

export function ReplaySession(): JSX.Element | null {
  const selection = useWorkspaceStore((state) => state.replaySelection);
  const action = useWorkspaceStore((state) => selection?.test ? state.testRun
    : state.recentActions.find((item) => item.id === selection?.actionId));
  const backend = useWorkspaceStore((state) => state.options.backend);
  const count = useWorkspaceStore((state) => Object.values(state.breakpoints).reduce((sum, lines) => sum + lines.length, 0));
  const event = selection && action?.events[selection.event];
  if (!selection || action?.id !== selection.actionId || event?.replay?.kind !== "ready") return <div className="editor-debug-idle" aria-label="Debugger">
    <Bug aria-hidden="true" size={14} />
    <span>{backend === "sonatina" ? "Debugger ready" : "Source debugging uses Sonatina"}</span>
    <span>{count ? `${count} breakpoint${count === 1 ? "" : "s"}` : "Click the gutter or press F9 to set a breakpoint"}</span>
  </div>;

  return <Suspense fallback={<p className="editor-replay__note" role="status">Opening replay…</p>}>
    <ReplayCall key={`${selection.test ? "test" : "call"}:${selection.actionId}:${selection.event}`} recording={event.replay} result={event.result}
      breakpoint={selection.breakpoint} testFailed={event.passed === false} />
  </Suspense>;
}
