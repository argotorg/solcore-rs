import { create } from "zustand";
import { compileClient } from "../compiler/compileClient";
import { nowMs } from "../compiler/timing";
import type { CompileInput, CompileResult, Diag } from "../compiler/types";
import { defaultExample, examples, getExample, type PlaygroundExample } from "../examples";

export interface WorkspaceFile {
  path: string;
  content: string;
}

export type OutputTab = "hull" | "yul" | "sonatina" | "problems";
export type ThemeMode = "light" | "dark";

interface WorkspaceOptions {
  emitHull: boolean;
  emitYul: boolean;
  emitSonatina: boolean;
  emitAbi: boolean;
}

export interface WorkspaceState {
  files: Record<string, WorkspaceFile>;
  order: string[];
  entry: string;
  activePath: string;
  compiling: boolean;
  compileStartedAt: number | null;
  lastCompileDurationMs: number | null;
  workspaceVersion: number;
  lastCompiledVersion: number | null;
  result: CompileResult | null;
  outputTab: OutputTab;
  theme: ThemeMode;
  options: WorkspaceOptions;
  setContent: (path: string, content: string) => void;
  createFile: (path: string) => string;
  renameFile: (from: string, to: string) => string | null;
  deleteFile: (path: string) => void;
  setEntry: (path: string) => void;
  setActive: (path: string) => void;
  setOutputTab: (tab: OutputTab) => void;
  toggleTheme: () => void;
  loadExample: (id: string) => void;
  resetWorkspace: () => void;
  compileNow: () => Promise<void>;
}

const WORKSPACE_STORAGE_KEY = "solcore-playground.workspace.v1";
const THEME_STORAGE_KEY = "solcore-playground.theme.v1";

let compileRun = 0;

function isBrowser(): boolean {
  return typeof window !== "undefined" && typeof document !== "undefined";
}

function normalizePath(path: string): string {
  const normalized = path.trim().replace(/\\/g, "/").replace(/^\/+/, "");
  return normalized.length > 0 ? normalized : "untitled.solc";
}

function ensureSolcExtension(path: string): string {
  return path.includes(".") ? path : `${path}.solc`;
}

function createFileMap(files: WorkspaceFile[]): Record<string, WorkspaceFile> {
  return Object.fromEntries(files.map((file) => [file.path, file]));
}

function cloneFiles(files: Array<{ path: string; content: string }>): WorkspaceFile[] {
  return files.map((file) => ({
    path: normalizePath(file.path),
    content: file.content,
  }));
}

function workspaceFromExample(example: PlaygroundExample): Pick<
  WorkspaceState,
  "files" | "order" | "entry" | "activePath"
> {
  const files = cloneFiles(example.files);
  const order = files.map((file) => file.path);
  const entry = normalizePath(example.entry);

  return {
    files: createFileMap(files),
    order,
    entry,
    activePath: entry,
  };
}

function makeUniquePath(path: string, existing: Record<string, WorkspaceFile>): string {
  const normalized = ensureSolcExtension(normalizePath(path));

  if (!existing[normalized]) {
    return normalized;
  }

  const slashIndex = normalized.lastIndexOf("/");
  const directory = slashIndex >= 0 ? `${normalized.slice(0, slashIndex + 1)}` : "";
  const fileName = slashIndex >= 0 ? normalized.slice(slashIndex + 1) : normalized;
  const dotIndex = fileName.lastIndexOf(".");
  const base = dotIndex >= 0 ? fileName.slice(0, dotIndex) : fileName;
  const extension = dotIndex >= 0 ? fileName.slice(dotIndex) : "";

  let index = 2;
  let candidate = `${directory}${base}-${index}${extension}`;
  while (existing[candidate]) {
    index += 1;
    candidate = `${directory}${base}-${index}${extension}`;
  }

  return candidate;
}

function applyTheme(theme: ThemeMode): void {
  if (!isBrowser()) {
    return;
  }

  document.documentElement.dataset.theme = theme;
}

function readInitialTheme(): ThemeMode {
  if (!isBrowser()) {
    return "dark";
  }

  const storedTheme = window.localStorage.getItem(THEME_STORAGE_KEY);
  if (storedTheme === "light" || storedTheme === "dark") {
    return storedTheme;
  }

  return window.matchMedia("(prefers-color-scheme: dark)").matches ? "dark" : "light";
}

interface PersistedWorkspace {
  files: WorkspaceFile[];
  order: string[];
  entry: string;
  activePath: string;
}

function readStoredWorkspace(): PersistedWorkspace | null {
  if (!isBrowser()) {
    return null;
  }

  try {
    const raw = window.localStorage.getItem(WORKSPACE_STORAGE_KEY);
    if (!raw) {
      return null;
    }

    const parsed = JSON.parse(raw) as Partial<PersistedWorkspace>;
    if (!Array.isArray(parsed.files) || parsed.files.length === 0) {
      return null;
    }

    const files = parsed.files
      .filter(
        (file): file is WorkspaceFile =>
          typeof file?.path === "string" && typeof file?.content === "string",
      )
      .map((file) => ({
        path: normalizePath(file.path),
        content: file.content,
      }));

    if (files.length === 0) {
      return null;
    }

    const fileSet = new Set(files.map((file) => file.path));
    const order = Array.isArray(parsed.order)
      ? parsed.order.map(normalizePath).filter((path) => fileSet.has(path))
      : [];

    for (const file of files) {
      if (!order.includes(file.path)) {
        order.push(file.path);
      }
    }

    const entry = typeof parsed.entry === "string" && fileSet.has(normalizePath(parsed.entry))
      ? normalizePath(parsed.entry)
      : order[0];
    const activePath =
      typeof parsed.activePath === "string" && fileSet.has(normalizePath(parsed.activePath))
        ? normalizePath(parsed.activePath)
        : entry;

    if (!entry || !activePath) {
      return null;
    }

    return { files, order, entry, activePath };
  } catch {
    return null;
  }
}

function persistWorkspace(state: WorkspaceState): void {
  if (!isBrowser()) {
    return;
  }

  const files = state.order
    .map((path) => state.files[path])
    .filter((file): file is WorkspaceFile => Boolean(file));

  const payload: PersistedWorkspace = {
    files,
    order: files.map((file) => file.path),
    entry: state.entry,
    activePath: state.activePath,
  };

  window.localStorage.setItem(WORKSPACE_STORAGE_KEY, JSON.stringify(payload));
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
    sonatina: null,
    abi: null,
  };
}

const storedWorkspace = readStoredWorkspace();
const initialWorkspace = storedWorkspace
  ? {
      files: createFileMap(storedWorkspace.files),
      order: storedWorkspace.order,
      entry: storedWorkspace.entry,
      activePath: storedWorkspace.activePath,
    }
  : workspaceFromExample(defaultExample);
const initialTheme = readInitialTheme();

applyTheme(initialTheme);

export const useWorkspaceStore = create<WorkspaceState>()((set, get) => ({
  ...initialWorkspace,
  compiling: false,
  compileStartedAt: null,
  lastCompileDurationMs: null,
  workspaceVersion: 0,
  lastCompiledVersion: null,
  result: null,
  outputTab: "hull",
  theme: initialTheme,
  options: {
    emitHull: true,
    emitYul: true,
    emitSonatina: true,
    emitAbi: false,
  },

  setContent(path, content) {
    const normalizedPath = normalizePath(path);

    set((state) => {
      const current = state.files[normalizedPath];
      if (!current || current.content === content) {
        return state;
      }

      return {
        files: {
          ...state.files,
          [normalizedPath]: {
            ...current,
            content,
          },
        },
        workspaceVersion: state.workspaceVersion + 1,
      };
    });

    persistWorkspace(get());
  },

  createFile(path) {
    const filePath = makeUniquePath(path, get().files);

    set((state) => ({
      files: {
        ...state.files,
        [filePath]: {
          path: filePath,
          content: "",
        },
      },
      order: [...state.order, filePath],
      activePath: filePath,
      workspaceVersion: state.workspaceVersion + 1,
    }));

    persistWorkspace(get());
    return filePath;
  },

  renameFile(from, to) {
    const fromPath = normalizePath(from);
    const state = get();
    const file = state.files[fromPath];

    if (!file) {
      return null;
    }

    const withoutSource = { ...state.files };
    delete withoutSource[fromPath];
    const toPath = makeUniquePath(to, withoutSource);

    set((currentState) => {
      const nextFiles = { ...currentState.files };
      delete nextFiles[fromPath];
      nextFiles[toPath] = {
        path: toPath,
        content: file.content,
      };

      return {
        files: nextFiles,
        order: currentState.order.map((path) => (path === fromPath ? toPath : path)),
        entry: currentState.entry === fromPath ? toPath : currentState.entry,
        activePath: currentState.activePath === fromPath ? toPath : currentState.activePath,
        workspaceVersion: currentState.workspaceVersion + 1,
      };
    });

    persistWorkspace(get());
    return toPath;
  },

  deleteFile(path) {
    const normalizedPath = normalizePath(path);
    const state = get();

    if (!state.files[normalizedPath] || state.order.length <= 1) {
      return;
    }

    const nextOrder = state.order.filter((filePath) => filePath !== normalizedPath);
    const nextFiles = { ...state.files };
    delete nextFiles[normalizedPath];
    const nextEntry = state.entry === normalizedPath ? nextOrder[0] : state.entry;
    const nextActive = state.activePath === normalizedPath ? nextEntry : state.activePath;

    if (!nextEntry || !nextActive) {
      return;
    }

    set({
      files: nextFiles,
      order: nextOrder,
      entry: nextEntry,
      activePath: nextActive,
      workspaceVersion: state.workspaceVersion + 1,
    });

    persistWorkspace(get());
  },

  setEntry(path) {
    const normalizedPath = normalizePath(path);
    const state = get();
    if (!state.files[normalizedPath]) {
      return;
    }

    set({
      entry: normalizedPath,
      activePath: normalizedPath,
      workspaceVersion:
        state.entry === normalizedPath ? state.workspaceVersion : state.workspaceVersion + 1,
    });

    persistWorkspace(get());
  },

  setActive(path) {
    const normalizedPath = normalizePath(path);
    if (!get().files[normalizedPath]) {
      return;
    }

    set({ activePath: normalizedPath });
    persistWorkspace(get());
  },

  setOutputTab(tab) {
    set({ outputTab: tab });
  },

  toggleTheme() {
    const nextTheme: ThemeMode = get().theme === "dark" ? "light" : "dark";
    set({ theme: nextTheme });
    applyTheme(nextTheme);

    if (isBrowser()) {
      window.localStorage.setItem(THEME_STORAGE_KEY, nextTheme);
    }
  },

  loadExample(id) {
    const nextWorkspace = workspaceFromExample(getExample(id));
    set((state) => ({
      ...nextWorkspace,
      result: null,
      lastCompileDurationMs: null,
      lastCompiledVersion: null,
      outputTab: "hull",
      workspaceVersion: state.workspaceVersion + 1,
    }));

    persistWorkspace(get());
  },

  resetWorkspace() {
    const nextWorkspace = workspaceFromExample(defaultExample);
    set((state) => ({
      ...nextWorkspace,
      result: null,
      lastCompileDurationMs: null,
      lastCompiledVersion: null,
      outputTab: "hull",
      workspaceVersion: state.workspaceVersion + 1,
    }));

    persistWorkspace(get());
  },

  async compileNow() {
    const runId = compileRun + 1;
    compileRun = runId;

    const state = get();
    const compileVersion = state.workspaceVersion;
    const startedAt = nowMs();
    const input: CompileInput = {
      files: state.order
        .map((path) => state.files[path])
        .filter((file): file is WorkspaceFile => Boolean(file))
        .map((file) => ({
          path: file.path,
          content: file.content,
        })),
      entry: state.entry,
      options: state.options,
    };

    set({ compiling: true, compileStartedAt: startedAt });

    try {
      const result = await compileClient.compile(input);
      const durationMs = nowMs() - startedAt;
      if (runId === compileRun) {
        set({
          result,
          compiling: false,
          compileStartedAt: null,
          lastCompileDurationMs: durationMs,
          lastCompiledVersion: compileVersion,
          outputTab: result.success ? get().outputTab : "problems",
        });
      }
    } catch (error: unknown) {
      if (error instanceof DOMException && error.name === "AbortError") {
        return;
      }

      if (runId === compileRun) {
        const durationMs = nowMs() - startedAt;
        set({
          result: diagnosticResult(error instanceof Error ? error.message : "Compile failed"),
          compiling: false,
          compileStartedAt: null,
          lastCompileDurationMs: durationMs,
          lastCompiledVersion: compileVersion,
          outputTab: "problems",
        });
      }
    }
  },
}));

export { examples };
