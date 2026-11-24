import type { StoreApi } from "zustand";
import { defaultExample, findExample, getExample, type PlaygroundExample } from "../examples";
import { readExampleRoute, readSharedExampleId } from "../share/exampleLink";
import { freshExecutionState, invalidateExecution } from "./executionSlice";
import { isBrowser } from "./isBrowser";
import { projectSource, workspaceStorage } from "./session";
import type { WorkspaceState } from "./workspace";

// Every Solcore source file must start with this declaration.
const NEW_SOURCE_FILE = "pragma solcore;\n\n";

function normalizePath(path: string): string {
  const normalized = path.trim().replace(/\\/g, "/").replace(/^\/+/, "");
  return normalized.length > 0 ? normalized : "untitled.sol";
}

function ensureSolExtension(path: string): string {
  return path.includes(".") ? path : `${path}.sol`;
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
  FilesSlice,
  "files" | "order" | "entry" | "activePath" | "exampleId"
> {
  const files = cloneFiles(example.files);
  const order = files.map((file) => file.path);
  const entry = normalizePath(example.entry);

  return {
    files: createFileMap(files),
    order,
    entry,
    activePath: entry,
    exampleId: example.id,
  };
}

function makeUniquePath(path: string, existing: Record<string, WorkspaceFile>): string {
  const normalized = ensureSolExtension(normalizePath(path));

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

interface PersistedWorkspace {
  files: WorkspaceFile[];
  order: string[];
  entry: string;
  activePath: string;
  exampleId: string;
  breakpoints?: Breakpoints;
}

function readStoredWorkspace(): PersistedWorkspace | null {
  if (!isBrowser()) {
    return null;
  }

  try {
    const raw = window.localStorage.getItem(workspaceStorage.key);
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

    const exampleId = projectSource?.id ?? (
      typeof parsed.exampleId === "string" && findExample(parsed.exampleId)
        ? parsed.exampleId
        : defaultExample.id);

    const breakpoints = Object.fromEntries(files.flatMap(({ path, content }) => {
      const lines = parsed.breakpoints?.[path];
      const count = content.split(/\r\n|\r|\n/).length;
      if (!Array.isArray(lines)) return [];
      const valid = [...new Set(lines.filter((line) => Number.isInteger(line) && line > 0 && line <= count))].sort((a, b) => a - b);
      return valid.length ? [[path, valid]] : [];
    }));
    return { files, order, entry, activePath, exampleId, breakpoints };
  } catch {
    return null;
  }
}

// One-slot backup: a shared example link overwrites the stored workspace on
// plain navigation, so the previous payload stays recoverable by hand.
export function backupStoredWorkspace(): void {
  if (!isBrowser()) {
    return;
  }

  try {
    const raw = window.localStorage.getItem(workspaceStorage.key);
    if (raw !== null) {
      window.localStorage.setItem(workspaceStorage.backupKey, raw);
    }
  } catch {
    // Best effort; never block loading the shared example.
  }
}

export function persistWorkspace(state: WorkspaceState): void {
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
    exampleId: state.exampleId,
    breakpoints: state.breakpoints,
  };

  try {
    window.localStorage.setItem(workspaceStorage.key, JSON.stringify(payload));
  } catch {
    // Editing and execution still work if storage is disabled or full.
  }
}

function readSharedExample(): PlaygroundExample | null {
  if (projectSource) return projectSource;
  if (!isBrowser()) {
    return null;
  }

  const sharedId = window.location.hash
    ? readExampleRoute(window.location.hash)
    : readSharedExampleId(window.location.search);
  return sharedId ? (findExample(sharedId) ?? null) : null;
}

export const sharedExample = readSharedExample();
export const savedWorkspace = readStoredWorkspace();
export const storedWorkspace = sharedExample && sharedExample.id !== savedWorkspace?.exampleId
  ? null
  : savedWorkspace;
export const initialWorkspace = storedWorkspace
  ? {
      files: createFileMap(storedWorkspace.files),
      order: storedWorkspace.order,
      entry: storedWorkspace.entry,
      activePath: storedWorkspace.activePath,
      exampleId: storedWorkspace.exampleId,
    }
  : workspaceFromExample(sharedExample ?? defaultExample);
export interface WorkspaceFile {
  path: string;
  content: string;
}

export type Breakpoints = Record<string, readonly number[]>;

export interface FilesSlice {
  files: Record<string, WorkspaceFile>;
  order: string[];
  entry: string;
  activePath: string;
  exampleId: string;
  workspaceVersion: number;
  breakpoints: Breakpoints;
  setBreakpoints: (path: string, lines: readonly number[]) => void;
  setContent: (path: string, content: string) => void;
  createFile: (path: string) => string;
  renameFile: (from: string, to: string) => string | null;
  deleteFile: (path: string) => void;
  setEntry: (path: string) => void;
  setActive: (path: string) => void;
  loadExample: (id: string) => void;
  resetWorkspace: () => void;
}

export function createFilesSlice(set: StoreApi<WorkspaceState>["setState"], get: StoreApi<WorkspaceState>["getState"]): FilesSlice {
  const replaceExample = (example: PlaygroundExample): void => {
    invalidateExecution();
    set((state) => ({
      ...workspaceFromExample(example),
      ...freshExecutionState(state.sandboxEpoch + 1, state.watchRevision),
      workspaceVersion: state.workspaceVersion + 1,
      outputTab: "execution",
      runActivity: "calls",
      viewedTestId: null,
      sourceSelection: null,
      replayLocation: null,
      breakpoints: {},
    }));
    persistWorkspace(get());
  };

  return {
    ...initialWorkspace,
    workspaceVersion: 0,
    breakpoints: storedWorkspace?.breakpoints ?? {},
    setBreakpoints(path, lines) {
      const state = get();
      if (!Object.hasOwn(state.files, path)) return;
      const previous = state.breakpoints[path] ?? [];
      if (lines.length === previous.length && lines.every((line, i) => line === previous[i])) return;
      const breakpoints = { ...state.breakpoints };
      if (lines.length) breakpoints[path] = lines;
      else delete breakpoints[path];
      set({ breakpoints });
      persistWorkspace(get());
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
            content: filePath.endsWith(".sol") ? NEW_SOURCE_FILE : "",
          },
        },
        order: [...state.order, filePath],
        activePath: filePath,
        sourceSelection: null,
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
        const breakpoints = { ...currentState.breakpoints };
        if (Object.hasOwn(breakpoints, fromPath)) {
          breakpoints[toPath] = breakpoints[fromPath];
          if (toPath !== fromPath) delete breakpoints[fromPath];
        }
        delete nextFiles[fromPath];
        nextFiles[toPath] = {
          path: toPath,
          content: file.content,
        };

        return {
          files: nextFiles,
          breakpoints,
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
      const breakpoints = { ...state.breakpoints };
      delete nextFiles[normalizedPath];
      delete breakpoints[normalizedPath];
      const nextEntry = state.entry === normalizedPath ? nextOrder[0] : state.entry;
      const nextActive = state.activePath === normalizedPath ? nextEntry : state.activePath;

      if (!nextEntry || !nextActive) {
        return;
      }

      set({
        files: nextFiles,
        breakpoints,
        order: nextOrder,
        entry: nextEntry,
        activePath: nextActive,
        sourceSelection: nextActive === state.activePath ? state.sourceSelection : null,
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
        sourceSelection: normalizedPath === state.activePath ? state.sourceSelection : null,
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

      set({ activePath: normalizedPath, ...(normalizedPath !== get().activePath ? { sourceSelection: null } : {}) });
      persistWorkspace(get());
    },

    loadExample(id) { if (!projectSource) replaceExample(getExample(id)); },
    resetWorkspace() { replaceExample(projectSource ?? defaultExample); },
  };
}
