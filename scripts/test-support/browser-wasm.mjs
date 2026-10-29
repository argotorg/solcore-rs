import { readFile } from 'node:fs/promises';
import NodeModule, { createRequire } from 'node:module';
import { dirname } from 'node:path';
import { fileURLToPath } from 'node:url';
import vm from 'node:vm';
import init, { compile, run, set_solc, watch } from '../../crates/wasm/pkg/solcore_wasm.js';
import { bindSolc } from '../../playground/src/compiler/solcBinding.js';
import { SOLJSON_FILE } from '../../playground/src/compiler/solcRelease.js';

export const backends = ['solc', 'sonatina'];

/** Load the same compiler WASM and pinned solc used by the browser runtime. */
export async function initializeBrowserWasm() {
  await init({
    module_or_path: await readFile(new URL('../../crates/wasm/pkg/solcore_wasm_bg.wasm', import.meta.url)),
  });
  // soljson is CommonJS, while the browser runtime package uses ES modules.
  const soljsonPath = fileURLToPath(new URL(`../../playground/public/solc/${SOLJSON_FILE}`, import.meta.url));
  const soljson = { exports: {} };
  vm.runInThisContext(NodeModule.wrap(await readFile(soljsonPath, 'utf8')), { filename: soljsonPath })(
    soljson.exports, createRequire(soljsonPath), soljson, soljsonPath, dirname(soljsonPath),
  );
  set_solc(bindSolc(soljson.exports).compile);
  return { compile, run, watch };
}
