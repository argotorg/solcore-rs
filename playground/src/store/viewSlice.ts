import type { StoreApi } from "zustand";
import type { WorkspaceState } from "./workspace";
import { isBrowser } from "./isBrowser";

const THEME_STORAGE_KEY = "solcore-playground.theme.v1";

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

export type OutputTab = "hull" | "yul" | "sonatina" | "abi" | "execution" | "problems";
export type ThemeMode = "light" | "dark";

export interface ViewSlice {
  outputTab: OutputTab;
  theme: ThemeMode;
  runActivity: "calls" | "tests";
  setOutputTab: (tab: OutputTab) => void;
  toggleTheme: () => void;
  setRunActivity: (activity: "calls" | "tests") => void;
}

export function createViewSlice(set: StoreApi<WorkspaceState>["setState"], get: StoreApi<WorkspaceState>["getState"]): ViewSlice {
  return {
    outputTab: "execution",
    theme: initialTheme,
    runActivity: "calls",
    setOutputTab(tab) { set({ outputTab: tab }); },
    setRunActivity(runActivity) { set({ runActivity }); },
    toggleTheme() {
      const nextTheme: ThemeMode = get().theme === "dark" ? "light" : "dark";
      set({ theme: nextTheme });
      applyTheme(nextTheme);
      if (isBrowser()) window.localStorage.setItem(THEME_STORAGE_KEY, nextTheme);
    },
  };
}

export const initialTheme = readInitialTheme();
applyTheme(initialTheme);
