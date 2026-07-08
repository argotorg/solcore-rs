import type { Pos } from "../compiler/types";

export interface EditorNavigationTarget {
  path: string;
  range: Pos;
}

type NavigationListener = (target: EditorNavigationTarget) => void;

const listeners = new Set<NavigationListener>();
let pendingTarget: EditorNavigationTarget | null = null;

export function requestEditorNavigation(target: EditorNavigationTarget): void {
  pendingTarget = target;
  for (const listener of listeners) {
    listener(target);
  }
}

export function subscribeEditorNavigation(listener: NavigationListener): () => void {
  listeners.add(listener);
  return () => {
    listeners.delete(listener);
  };
}

export function consumePendingNavigation(path: string): EditorNavigationTarget | null {
  if (pendingTarget?.path !== path) {
    return null;
  }

  const target = pendingTarget;
  pendingTarget = null;
  return target;
}
