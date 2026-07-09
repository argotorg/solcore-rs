import { loader } from "@monaco-editor/react";
import * as monaco from "monaco-editor";
import EditorWorker from "monaco-editor/esm/vs/editor/editor.worker?worker";

type MonacoEnvironment = {
  getWorker: (workerId: string, label: string) => Worker;
};

const monacoScope = self as typeof self & {
  MonacoEnvironment?: MonacoEnvironment;
};

monacoScope.MonacoEnvironment = {
  getWorker() {
    return new EditorWorker();
  },
};

// Use the locally bundled monaco-editor (matching the pinned types and the
// bundled editor worker) instead of @monaco-editor/react's default CDN build.
// The CDN build drifts to a newer major (0.55.x) whose editor bundle omits the
// go-to-definition / find-references actions and mismatches the 0.52 worker,
// which broke LSP navigation. Bundling keeps main thread + worker + types in
// lockstep and makes the playground fully offline.
loader.config({ monaco });
