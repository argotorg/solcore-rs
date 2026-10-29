import assert from "node:assert/strict";
import test from "node:test";
import { fileURLToPath } from "node:url";
import { build } from "esbuild";

test("backend loading preserves run and watch order and recovers after failure", async () => {
  let receive;
  let releaseSolc;
  const loading = new Promise((resolve) => { releaseSolc = resolve; });
  const executed = [];
  const responses = [];
  let deployment;
  let solcUrl;
  const runtimeUrl = "https://tutorial.example/course/playground/solc/soljson.js";
  const originalSelf = globalThis.self;
  globalThis.self = {
    addEventListener(_name, listener) { receive = listener; },
    postMessage(response) { responses.push(response); },
  };
  globalThis.__solcoreWorkerAuditRuntime = {
    configureSolcUrl: (url) => { solcUrl = url; },
    initializeCompiler: async () => {},
    initializeSolc: () => {
      assert.equal(solcUrl, runtimeUrl, "the bundled runtime must be configured before loading");
      return loading;
    },
    needsSolc: (input) => input.backend === "solc",
    compile: () => { throw new Error("invalid compile request"); },
    run: (input) => {
      deployment = input.contract;
      executed.push(deployment);
      return { contract: deployment };
    },
    watch: () => [{ value: deployment }],
  };
  try {
    const bundle = await build({
      entryPoints: [fileURLToPath(new URL("./compile.worker.ts", import.meta.url))],
      bundle: true, write: false, format: "esm", platform: "node",
      plugins: [{ name: "runtime", setup(api) {
        api.onResolve({ filter: /^\.\/runtime$/ }, () => ({ path: "runtime", namespace: "mock" }));
        api.onLoad({ filter: /.*/, namespace: "mock" }, () => ({
          contents: "export const { configureSolcUrl, initializeCompiler, initializeSolc, needsSolc, compile, run, watch } = globalThis.__solcoreWorkerAuditRuntime;",
        }));
      } }],
    });
    await import(`data:text/javascript;base64,${Buffer.from(bundle.outputFiles[0].text).toString("base64")}`);
    receive({ data: { kind: "configure-runtime", solcUrl: runtimeUrl } });
    assert.equal(solcUrl, runtimeUrl);
    assert.deepEqual(responses, [], "configuration must not produce a compile response");
    receive({ data: { id: 1, kind: "run", input: { backend: "solc", contract: "old" } } });
    receive({ data: { id: 2, kind: "run", input: { backend: "sonatina", contract: "current" } } });
    receive({ data: { id: 3, kind: "watch", input: {} } });
    await new Promise((resolve) => setImmediate(resolve));
    assert.deepEqual(executed, [], "later runs must wait for the earlier backend load");
    releaseSolc();
    await new Promise((resolve) => setImmediate(resolve));
    assert.deepEqual(executed, ["old", "current"]);
    assert.deepEqual(responses.map((r) => r.id), [1, 2, 3]);
    assert.deepEqual(responses[2].result, [{ value: "current" }]);
    receive({ data: { id: 4, kind: "compile", input: {} } });
    receive({ data: { id: 5, kind: "run", input: { contract: "after-error" } } });
    await new Promise((resolve) => setImmediate(resolve));
    assert.equal(responses[3].kind, "error");
    assert.equal(responses[4].result.contract, "after-error");
  } finally {
    releaseSolc();
    globalThis.self = originalSelf;
    delete globalThis.__solcoreWorkerAuditRuntime;
  }
});
