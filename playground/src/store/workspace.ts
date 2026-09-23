import { create } from "zustand";
import { examples } from "../examples";
import { createFilesSlice, sharedExample, storedWorkspace, backupStoredWorkspace, persistWorkspace, type FilesSlice } from "./filesSlice";
import { createExecutionSlice, type ExecutionSlice } from "./executionSlice";
import { createViewSlice, type ViewSlice } from "./viewSlice";

export type { WorkspaceFile } from "./filesSlice";
export type { OutputTab, ThemeMode } from "./viewSlice";
export type WorkspaceState = FilesSlice & ViewSlice & ExecutionSlice;

export const useWorkspaceStore = create<WorkspaceState>()((set, get) => ({
  ...createFilesSlice(set, get),
  ...createViewSlice(set, get),
  ...createExecutionSlice(set, get),
}));

if (sharedExample && !storedWorkspace) {
  backupStoredWorkspace();
  persistWorkspace(useWorkspaceStore.getState());
}

export { examples };
