import { create } from "zustand";
import { examples } from "../examples";
import { stripExampleParam } from "../share/exampleLink";
import { createFilesSlice, sharedExample, backupStoredWorkspace, persistWorkspace, type FilesSlice } from "./filesSlice";
import { createExecutionSlice, type ExecutionSlice } from "./executionSlice";
import { createViewSlice, type ViewSlice } from "./viewSlice";
import { isBrowser } from "./isBrowser";

export type { WorkspaceFile } from "./filesSlice";
export type { OutputTab, ThemeMode } from "./viewSlice";
export type WorkspaceState = FilesSlice & ViewSlice & ExecutionSlice;

export const useWorkspaceStore = create<WorkspaceState>()((set, get) => ({
  ...createFilesSlice(set, get),
  ...createViewSlice(set, get),
  ...createExecutionSlice(set, get),
}));

if (sharedExample) {
  // The shared example replaces any locally stored workspace: back the previous
  // payload up, persist the example, and drop the parameter so a later reload
  // keeps the visitor's edits instead of resetting the example. An unknown id
  // deliberately keeps the parameter in the address bar, so a mistyped link
  // stays diagnosable from a screenshot.
  backupStoredWorkspace();
  persistWorkspace(useWorkspaceStore.getState());

  if (isBrowser()) {
    window.history.replaceState(null, "", stripExampleParam(window.location.href));
  }
}

export { examples };
