import { access } from "node:fs/promises";
import { fileURLToPath } from "node:url";
import { previewStaticSite } from "../../playground/scripts/build.mjs";

try {
  const outDir = fileURLToPath(new URL("../book", import.meta.url));
  await access(new URL("../book/index.html", import.meta.url)).catch(() => {
    throw new Error("Build the tutorial with npm run build:book before starting the preview.");
  });
  const port = Number(process.env.PORT ?? 4177);
  if (!Number.isInteger(port) || port < 1 || port > 65535) throw new Error("PORT must be an integer between 1 and 65535.");
  const server = await previewStaticSite({ outDir, port });
  server.printUrls();
} catch (error) {
  console.error(error.message);
  process.exitCode = 1;
}
