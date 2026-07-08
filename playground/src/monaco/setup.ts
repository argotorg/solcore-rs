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
