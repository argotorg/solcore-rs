import { useEffect, useMemo, useState, type ComponentProps } from "react";
import { TraceProgress, TraceProvider, useTraceContext } from "@ethdebug/programs-react";
import "@ethdebug/programs-react/dist/src/components/TraceControls.css";
import { ChevronLeft, ChevronRight, Play, Square } from "lucide-react";
import type { ExecutionResult, RecordedCall } from "../compiler/types";
import { breakpointSteps, replayPositions } from "../compiler/replay";
import { useWorkspaceStore } from "../store/workspace";

type Program = ComponentProps<typeof TraceProvider>["program"];
function replayView(recording: RecordedCall) {
  const program = JSON.parse(recording.program.json) as Program;
  const instructions = new Map(program.instructions?.map((instruction) => [instruction.offset, instruction]));
  const sources = replayPositions(recording);
  const steps = recording.steps.map(({ pc, opcode }, index) => ({
    trace: { pc, opcode: instructions.get(pc)?.operation?.mnemonic ?? `0x${opcode.toString(16).padStart(2, "0")}` },
    source: sources[index],
  }));
  const visits: number[] = [];
  const projectVisits: number[] = [];
  for (const [index, source] of sources.entries()) {
    if (!source) continue;
    if (source !== sources[visits.at(-1) ?? -1]) visits.push(index);
    if (!source.range.file.startsWith("std:") && source !== sources[projectVisits.at(-1) ?? -1]) projectVisits.push(index);
  }
  return { program, trace: steps.map((step) => step.trace), steps, sources, visits: projectVisits.length ? projectVisits : visits };
}

function ReplayToolbar({ view, outcome, failed, incomplete, lastMapped, initialBreakpoint }: {
  view: ReturnType<typeof replayView>;
  outcome: string;
  failed: boolean;
  incomplete: string | null;
  lastMapped: number | undefined;
  initialBreakpoint: number | undefined;
}): JSX.Element {
  const { currentStepIndex, jumpToStep } = useTraceContext();
  const [mode, setMode] = useState(view.visits.length ? "source" : "instruction");
  const step = view.steps[currentStepIndex];
  const source = step?.source;
  const breakpoints = useWorkspaceStore((state) => state.breakpoints);
  const files = useWorkspaceStore((state) => state.files);
  const hits = useMemo(() => breakpointSteps(view.sources, breakpoints, files), [view.sources, breakpoints, files]);
  const atBreakpoint = hits.includes(currentStepIndex) || initialBreakpoint === currentStepIndex;
  const terminal = view.steps.length - 1;
  const destination = hits.find((index) => index > currentStepIndex) ?? terminal;
  const setLocation = useWorkspaceStore((state) => state.setReplayLocation);
  const canFollow = useWorkspaceStore((state) => Boolean(source && state.files[source.range.file]?.content === source.contents));
  useEffect(() => () => setLocation(null), [setLocation]);
  useEffect(() => { setLocation(canFollow ? source : null); }, [setLocation, source, canFollow, currentStepIndex]);

  let previous: number | undefined;
  let next: number | undefined;
  if (mode === "source") {
    for (let visit = view.visits.length - 1; visit >= 0; visit--) {
      const index = view.visits[visit];
      if (index < currentStepIndex && view.steps[index].source !== source) {
        previous = index;
        break;
      }
    }
    next = view.visits.find((index) => index > currentStepIndex && view.steps[index].source !== source);
  } else {
    if (currentStepIndex > 0) previous = currentStepIndex - 1;
    if (currentStepIndex + 1 < view.steps.length) next = currentStepIndex + 1;
  }

  const position = source ? `${source.range.file}:${source.range.startLine}` : "No mapped source";
  const locationStatus = source && !canFollow ? position + (source.range.file.startsWith("std:") ? " · Library source" : " · Recorded source differs")
    : atBreakpoint ? `Breakpoint · ${position}`
    : failed && currentStepIndex === lastMapped ? `Last mapped source · ${position}`
    : currentStepIndex === terminal && !source ? `End of recording · ${position}` : position;
  const instruction = step ? `Step ${currentStepIndex + 1}/${view.steps.length} · ${step.trace.opcode} · PC ${step.trace.pc}` : "";

  return <section className="editor-replay" aria-label="Call replay">
    <div className="editor-replay__toolbar" role="group" aria-label="Replay controls">
      <button type="button" className="button button--ghost" aria-label="Continue replay" title="Continue to the next breakpoint or the end"
        aria-disabled={currentStepIndex === terminal} onClick={() => currentStepIndex !== terminal && jumpToStep(destination)}>
        <Play size={16} aria-hidden="true" />
      </button>
      <select aria-label="Step by" value={mode} onChange={(event) => setMode(event.target.value)}>
        <option value="source" disabled={!view.visits.length}>Source</option>
        <option value="instruction">Instruction</option>
      </select>
      <button type="button" className="button button--ghost" aria-label="Step back" title="Step back" aria-disabled={previous === undefined}
        onClick={() => previous !== undefined && jumpToStep(previous)}><ChevronLeft size={17} aria-hidden="true" /></button>
      <button type="button" className="button button--ghost" aria-label="Step forward" title="Step forward" aria-disabled={next === undefined}
        onClick={() => next !== undefined && jumpToStep(next)}><ChevronRight size={17} aria-hidden="true" /></button>
      <button type="button" className="button button--ghost" aria-label="Close replay" title="Close replay"
        onClick={() => useWorkspaceStore.setState({ replaySelection: null })}><Square size={13} aria-hidden="true" /></button>
    </div>
    <div className="editor-replay__status" aria-live="polite">
      <strong className={failed ? "run-pane__error" : undefined} title={outcome}>{outcome}</strong>
      <span title={locationStatus}>{locationStatus}</span>
    </div>
    {incomplete ? <p className="editor-replay__note">{incomplete}</p> : null}
    {mode === "instruction" ? <div className="editor-replay__instruction">
      <TraceProgress className="editor-replay__rail" />
      <code title={instruction}>{instruction}</code>
    </div> : null}
  </section>;
}

export default function ReplayCall({ recording, result, breakpoint, testFailed }: {
  recording: RecordedCall;
  result: ExecutionResult;
  breakpoint?: number;
  testFailed?: boolean;
}): JSX.Element {
  const view = useMemo(() => replayView(recording), [recording]);
  const failed = testFailed || result.status !== "success";
  const lastMapped = view.visits.at(-1);
  const initial = breakpoint ?? (failed ? lastMapped ?? view.trace.length - 1 : view.visits[0] ?? 0);
  const outcome = result.status !== "success" ? result.message ?? "Execution failed"
    : `${testFailed ? "Test failed · returned" : "Returned"} ${result.decoded ?? result.returnWord ?? "successfully"}`;
  return <TraceProvider trace={view.trace} program={view.program} resolveVariables={false} initialStepIndex={initial}>
    <ReplayToolbar view={view} outcome={outcome} failed={failed} incomplete={recording.incomplete}
      lastMapped={lastMapped} initialBreakpoint={breakpoint} />
  </TraceProvider>;
}
