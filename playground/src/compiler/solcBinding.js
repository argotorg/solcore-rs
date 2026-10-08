/**
 * Binds the emscripten solc module (soljson) to a Standard JSON compile function.
 *
 * soljson is built with synchronous WebAssembly compilation, so the module is
 * ready as soon as its script has run.
 */
export function bindSolc(soljson) {
  if (!soljson || typeof soljson.cwrap !== "function") {
    throw new Error("soljson did not define an emscripten module");
  }
  const compile = soljson.cwrap("solidity_compile", "string", ["string", "number", "number"]);
  const reset = soljson.cwrap("solidity_reset", null, []);
  const version = soljson.cwrap("solidity_version", "string", []);
  return {
    version: version(),
    compile(input) {
      try {
        return compile(input, 0, 0);
      } finally {
        // Frees the returned string and all other compiler allocations.
        reset();
      }
    },
  };
}
