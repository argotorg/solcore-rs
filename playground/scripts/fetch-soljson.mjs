// Downloads the pinned soljson build into public/solc/ and verifies its sha256.
import { createHash } from "node:crypto";
import { mkdir, readFile, writeFile } from "node:fs/promises";
import { SOLJSON_FILE, SOLJSON_SHA256 } from "../src/compiler/solcRelease.js";

const target = new URL(`../public/solc/${SOLJSON_FILE}`, import.meta.url);
const source = `https://binaries.soliditylang.org/wasm/${encodeURIComponent(SOLJSON_FILE)}`;
const sha256 = (bytes) => createHash("sha256").update(bytes).digest("hex");

try {
  if (sha256(await readFile(target)) === SOLJSON_SHA256) process.exit(0);
} catch (error) {
  if (error.code !== "ENOENT") throw error;
}

console.log(`Downloading ${source}`);
const response = await fetch(source);
if (!response.ok) throw new Error(`Downloading ${source} failed with HTTP ${response.status}`);
const bytes = Buffer.from(await response.arrayBuffer());
const actual = sha256(bytes);
if (actual !== SOLJSON_SHA256) {
  throw new Error(`${SOLJSON_FILE} has sha256 ${actual}, expected ${SOLJSON_SHA256}`);
}
await mkdir(new URL("./", target), { recursive: true });
await writeFile(target, bytes);
