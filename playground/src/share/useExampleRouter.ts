import { useEffect } from "react";
import { findExample } from "../examples";
import { useWorkspaceStore } from "../store/workspace";
import { buildExampleLink, readExampleRoute, readSharedExampleId } from "./exampleLink";

export function useExampleRouter(): void {
  useEffect(() => {
    const followRoute = (): void => {
      const id = readExampleRoute(window.location.hash);
      const state = useWorkspaceStore.getState();
      if (id && findExample(id) && id !== state.exampleId) {
        state.loadExample(id);
      }
    };

    // Keep unknown links intact. Normalize empty and legacy URLs without adding history.
    const legacyId = readSharedExampleId(window.location.search);
    if (!window.location.hash && (!legacyId || findExample(legacyId))) {
      window.history.replaceState(
        null,
        "",
        buildExampleLink(window.location.href, useWorkspaceStore.getState().exampleId),
      );
    }
    followRoute();
    window.addEventListener("hashchange", followRoute);
    const unsubscribe = useWorkspaceStore.subscribe((state, previous) => {
      if (
        state.exampleId !== previous.exampleId &&
        readExampleRoute(window.location.hash) !== state.exampleId
      ) {
        window.history.pushState(null, "", buildExampleLink(window.location.href, state.exampleId));
      }
    });
    return () => {
      window.removeEventListener("hashchange", followRoute);
      unsubscribe();
    };
  }, []);
}
