import type { StoreApi } from "zustand";
import { compileClient } from "../compiler/compileClient";
import { ACCOUNTS } from "../compiler/accounts";
import { formatCall } from "../compiler/formatCall";
import { breakpointSteps, replayPositions } from "../compiler/replay";
import { nowMs } from "../compiler/timing";
import type { Backend, CompileInput, CompileResult, Diag, TestCase, ContractInterface, ManualCall, WatchCall, WatchResult, RecentAction } from "../compiler/types";
import type { WorkspaceState } from "./workspace";
import type { Breakpoints, WorkspaceFile } from "./filesSlice";

let compileRun = 0;

function keepRecentCalls(actions: RecentAction[]): RecentAction[] {
  let recordings = 0;
  return actions.slice(-20).reverse().map((action) => {
    const event = action.events.find((event) => event.replay?.kind === "ready");
    if (!event || ++recordings <= 2) return action;
    return {
      ...action,
      events: action.events.map((item) => item === event ? {
        ...item,
        replay: { kind: "unavailable", message: "This recording expired." } as const,
      } : item),
    };
  }).reverse();
}
let watchRun = 0;

export function invalidateExecution(): void {
  compileRun += 1;
}

function diagnosticResult(message: string): CompileResult {
  const diagnostic: Diag = {
    severity: "error",
    code: "CLIENT",
    message,
    primary: null,
    labels: [],
    notes: [],
    helps: [],
  };

  return {
    success: false,
    diagnostics: [diagnostic],
    hull: null,
    yul: null,
    yulOutputs: [],
    sonatina: null,
    abi: null,
    bytecode: [],
    execution: null,
    tests: [],
    contracts: [],
    hasMain: false,
    sandbox: null,
    events: [],
  };
}

export interface ExecutionData {
  contracts: ContractInterface[];
  hasMain: boolean;
  discoveryVersion: number | null;
  callDraft: ManualCall;
  selectedTestId: string | null;
  sandbox: CompileResult["sandbox"];
  sandboxVersion: number | null;
  sandboxEpoch: number;
  manualResult: CompileResult["execution"];
  manualResultVersion: number | null;
  recentActions: RecentAction[];
  replaySelection: ReplaySelection | null;
  testRun: RecentAction | null;
  actionSequence: number;
  sandboxAction: number | null;
  watchAction: number | null;
  watches: WatchCall[];
  watchValues: Record<string, WatchResult & { changed: boolean }>;
  watchVersion: number | null;
  watchEpoch: number | null;
  watchRevision: number;
  watchLoading: boolean;
  testCases: TestCase[];
  testResults: TestCase[];
  testResultsVersion: number | null;
  compiling: boolean;
  running: boolean;
  compileStartedAt: number | null;
  lastCompileDurationMs: number | null;
  lastCompiledVersion: number | null;
  result: CompileResult | null;
}

export interface ExecutionOptions {
  emitHull: boolean;
  emitYul: boolean;
  emitSonatina: boolean;
  emitAbi: boolean;
  emitBytecode: boolean;
  backend: Backend;
}

export interface ReplaySelection {
  actionId: number;
  event: number;
  breakpoint?: number;
  test?: boolean;
}

function initialReplay(action: RecentAction, files: Record<string, WorkspaceFile>, breakpoints: Breakpoints, test: boolean): ReplaySelection | null {
  const hasBreakpoints = Object.keys(breakpoints).length > 0;
  for (const [index, event] of action.events.entries()) {
    if (event.replay?.kind !== "ready") continue;
    const breakpoint = hasBreakpoints ? breakpointSteps(replayPositions(event.replay), breakpoints, files)[0] : undefined;
    const failed = event.passed === false || (event.passed === null && event.result.status !== "success");
    if (breakpoint !== undefined || failed) return { actionId: action.id, event: index, breakpoint, ...(test ? { test } : {}) };
  }
  return null;
}

export interface ExecutionSlice extends ExecutionData {
  options: ExecutionOptions;
  setBackend: (backend: Backend) => void;
  setCallDraft: (patch: Partial<ManualCall>) => void;
  runCall: (simulate?: boolean) => Promise<void>;
  resetSandbox: () => void;
  addWatch: (call: Omit<WatchCall, "id">) => void;
  removeWatch: (id: string) => void;
  refreshWatches: () => Promise<void>;
  compileNow: () => Promise<void>;
  runNow: (testId?: string) => Promise<void>;
}

export function freshExecutionState(sandboxEpoch = 0, watchRevision = 0): ExecutionData {
  return {
    contracts: [], hasMain: false, discoveryVersion: null,
    callDraft: { contract: "", signature: "", arguments: "[]", constructorArguments: "[]", simulate: true, caller: ACCOUNTS[0] },
    selectedTestId: null,
    testRun: null, recentActions: [], replaySelection: null, actionSequence: 0, sandboxAction: null, watchAction: null,
    sandbox: null, sandboxVersion: null, sandboxEpoch,
    manualResult: null, manualResultVersion: null,
    watches: [], watchValues: {}, watchVersion: null, watchEpoch: null,
    watchRevision, watchLoading: false,
    testCases: [], testResults: [], testResultsVersion: null,
    compiling: false, running: false, compileStartedAt: null, lastCompileDurationMs: null,
    lastCompiledVersion: null, result: null,
  };
}

export function createExecutionSlice(set: StoreApi<WorkspaceState>["setState"], get: StoreApi<WorkspaceState>["getState"]): ExecutionSlice {
  async function executeWorkspace(run: boolean, testId?: string, manual?: ManualCall): Promise<void> {
    const runId = compileRun + 1;
    compileRun = runId;

    const state = get();
    const compileVersion = state.workspaceVersion;
    const startedAt = nowMs();
    const testing = run && !manual && (Boolean(testId) || state.testCases.length > 0);
    const recordTrace = run && state.options.backend === "sonatina";
    const input: CompileInput = {
      files: state.order
        .map((path) => state.files[path])
        .filter((file): file is WorkspaceFile => Boolean(file))
        .map((file) => ({
          path: file.path,
          content: file.content,
        })),
      entry: state.entry,
      options: { ...state.options, recordTrace: recordTrace && !manual,
        ...(testing && recordTrace ? { breakpoints: state.breakpoints } : {}) },
      testId,
      manual,
      sandboxEpoch: state.sandboxEpoch + state.workspaceVersion,
    };

    if (run && !testing) watchRun += 1;
    const selected = testId ? state.testCases.find((t) => t.id === testId) : null;
    set({
      ...(run ? {
        replaySelection: null, replayLocation: null,
        ...(!testing ? { watchLoading: false } : {}),
        selectedTestId: testId ?? null,
        ...(testId ? { viewedTestId: testId } : {}),
        runActivity: testing ? "tests" as const : "calls" as const,
        ...(testing ? { testRun: null } : { manualResult: null, manualResultVersion: null }),
        outputTab: "execution" as const,
        ...(selected?.invocation ? { callDraft: {
          contract: selected.contract, constructorArguments: state.callDraft.contract === selected.contract ? state.callDraft.constructorArguments : "[]",
          caller: state.callDraft.caller, ...selected.invocation,
        } } : {}),
      } : {}),
      compiling: true,
      running: run,
      ...(testing ? { testResults: [], testResultsVersion: null } : {}),
      compileStartedAt: startedAt,
      ...(run && state.result ? { result: { ...state.result, execution: null } } : {}),
    });

    try {
      const result = await (run ? compileClient.run(input) : compileClient.compile(input));
      const durationMs = nowMs() - startedAt;
      if (runId === compileRun) {
        const actionId = testing ? (state.testRun?.id ?? 0) + 1 : get().actionSequence + 1;
        const label = manual
          ? `${manual.simulate ? "Preview" : "Call"} ${manual.contract}.${formatCall(manual.signature, manual.arguments)}`
          : selected ? `Run test · ${selected.file}:${selected.line}`
          : input.testId ? "Run selected test" : result.tests.length ? "Run all tests" : "Run main";
        const action: RecentAction = { id: actionId, label, version: compileVersion,
          sandboxId: testing ? null : result.sandbox?.id ?? null, events: result.events ?? [], result: result.execution,
          message: result.execution?.message ?? result.diagnostics.find((d) => d.severity === "error")?.message ?? null };
        const current = get();
        const recentActions = run && !testing ? keepRecentCalls([...current.recentActions, action]) : current.recentActions;
        set({
          ...(testing ? { testRun: action } : {}),
          ...(run ? { replaySelection: compileVersion === current.workspaceVersion
            ? initialReplay(action, state.files, state.breakpoints, testing) : null } : {}),
          ...(run && !testing ? {
            actionSequence: actionId,
            sandboxAction: result.sandbox ? actionId : null,
            recentActions,
          } : {}),
          result,
          ...(compileVersion === get().workspaceVersion ? { hasMain: result.hasMain ?? false, discoveryVersion: compileVersion } : {}),
          contracts: compileVersion === get().workspaceVersion ? (result.contracts ?? []) : get().contracts,
          ...(run && !testing ? {
            sandbox: result.sandbox ?? null,
            sandboxVersion: compileVersion,
            ...(manual ? { manualResult: result.execution, manualResultVersion: compileVersion } : {}),
          } : {}),
          testCases: compileVersion === get().workspaceVersion ? result.tests : get().testCases,
          ...(testing ? { testResults: result.tests, testResultsVersion: compileVersion } : {}),
          compiling: false,
          running: false,
          compileStartedAt: null,
          lastCompileDurationMs: durationMs,
          lastCompiledVersion: compileVersion,
          outputTab: compileVersion !== get().workspaceVersion
            ? get().outputTab
            : result.success ? (run ? "execution" : get().outputTab) : "problems",
        });
        if (run && !testing) void get().refreshWatches();
      }
    } catch (error: unknown) {
      if (error instanceof DOMException && error.name === "AbortError") {
        return;
      }

      if (runId === compileRun) {
        const durationMs = nowMs() - startedAt;
        const message = error instanceof Error ? error.message : "Compile failed";
        set({
          ...(testing ? { testRun: { id: 0, label: "Test run failed", version: compileVersion,
            sandboxId: null, events: [], result: null, message } } : {}),
          ...(run && !testing ? {
            actionSequence: get().actionSequence + 1,
            recentActions: [...get().recentActions, { id: get().actionSequence + 1,
              label: manual ? `${manual.simulate ? "Preview" : "Call"} ${manual.contract}.${formatCall(manual.signature, manual.arguments)}` : "Run failed",
              version: compileVersion, sandboxId: null, events: [], result: null, message,
            }].slice(-20),
          } : {}),
          result: diagnosticResult(message),
          compiling: false,
          running: false,
          compileStartedAt: null,
          lastCompileDurationMs: durationMs,
          lastCompiledVersion: compileVersion,
          outputTab: compileVersion === get().workspaceVersion ? "problems" : get().outputTab,
        });
      }
    }
  }

  async function refreshWatches(): Promise<void> {
    const state = get();
    const requestId = ++watchRun;
    const watches = state.watches.filter((watch) => watch.contract === state.sandbox?.contract);
    if (state.compiling || !state.sandbox || state.sandboxVersion !== state.workspaceVersion || !watches.length) {
      set({ watchLoading: false });
      return;
    }
    const current = (): boolean => requestId === watchRun
      && state.workspaceVersion === get().workspaceVersion
      && state.sandboxEpoch === get().sandboxEpoch
      && state.sandbox?.id === get().sandbox?.id;
    set({ watchLoading: true });
    try {
      const results = await compileClient.watch({
        workspace: {
          files: state.order.map((path) => state.files[path]), entry: state.entry,
          options: state.options, sandboxEpoch: state.sandboxEpoch + state.workspaceVersion,
        }, watches,
      });
      if (!current()) return;
      set({
        watchValues: Object.fromEntries(results.filter((result) => get().watches.some((w) => w.id === result.id)).map((result) => [result.id, {
          ...result, changed: result.value !== null && state.watchValues[result.id]?.value != null
            && result.value !== state.watchValues[result.id].value,
        }])),
        watchAction: state.sandboxAction,
        watchVersion: state.workspaceVersion, watchEpoch: state.sandboxEpoch,
        watchRevision: state.watchRevision + 1,
      });
    } catch (error: unknown) {
      if (!current() || error instanceof DOMException && error.name === "AbortError") return;
      const message = error instanceof Error ? error.message : "Could not read watches";
      set({ watchValues: Object.fromEntries(watches.map((watch) => [watch.id, {
        id: watch.id, value: null, error: message, changed: false,
      }])), watchVersion: state.workspaceVersion, watchEpoch: state.sandboxEpoch });
    } finally {
      if (requestId === watchRun) set({ watchLoading: false });
    }
  }

  return {
    ...freshExecutionState(),
    options: {
      emitHull: true,
      emitYul: true,
      emitSonatina: true,
      emitAbi: true,
      emitBytecode: true,
      backend: "sonatina",
    },
    setBackend(backend) {
      if (get().options.backend === backend) return;
      // Outputs and deployments from the other backend no longer apply.
      invalidateExecution();
      get().resetSandbox();
      set((s) => ({
        // Sonatina IR is only shown, and so only generated, with the Sonatina backend.
        options: { ...s.options, backend, emitSonatina: backend === "sonatina" },
        ...(s.outputTab === "sonatina" && backend !== "sonatina" ? { outputTab: "execution" as const } : {}),
        result: null, lastCompiledVersion: null, lastCompileDurationMs: null,
        compiling: false, running: false, compileStartedAt: null,
        testResults: [], testResultsVersion: null, testRun: null, replaySelection: null, replayLocation: null,
      }));
    },
    setCallDraft(patch) { set((s) => ({ callDraft: { ...s.callDraft, ...patch }, selectedTestId: null })); },
    runCall: (simulate = false) => executeWorkspace(true, undefined,
      { ...get().callDraft, simulate, recordTrace: get().options.backend === "sonatina" }),
    resetSandbox() {
      watchRun += 1;
      set((s) => ({ sandbox: null, sandboxVersion: null, sandboxEpoch: s.sandboxEpoch + 1,
        sandboxAction: null, watchAction: null, manualResult: null, manualResultVersion: null,
        actionSequence: 0,
        recentActions: [], replaySelection: s.replaySelection?.test ? s.replaySelection : null,
        replayLocation: s.replaySelection?.test ? s.replayLocation : null,
        watchValues: {}, watchVersion: null, watchEpoch: null, watchLoading: false }));
    },
    addWatch(call) {
      const normalized = { ...call, arguments: call.arguments.trim() };
      const id = JSON.stringify([normalized.contract, normalized.signature, normalized.arguments]);
      if (get().watches.length >= 16 || get().watches.some((watch) => watch.id === id)) return;
      set((s) => ({ watches: [...s.watches, { ...normalized, id }] }));
      void get().refreshWatches();
    },
    removeWatch(id) {
      set((s) => ({ watches: s.watches.filter((watch) => watch.id !== id) }));
    },
    refreshWatches,

    compileNow: () => executeWorkspace(false),
    runNow: (testId) => {
      const state = get();
      if (!testId && !state.testCases.length) {
        if (state.contracts.some((contract) => contract.methods.length > 0)) {
          set({ outputTab: "execution", runActivity: "calls" });
          return Promise.resolve();
        }
      }
      return executeWorkspace(true, testId);
    },
  };
}
