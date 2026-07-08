# Solcore LSP Web Worker

`worker.ts` adapts the wasm-bindgen `SolcoreLsp` entry to a dedicated browser
worker. The transport is raw LSP JSON-RPC 2.0 objects over `postMessage`; there
is no `Content-Length` framing because worker messages are already discrete.

- Positions are the LSP-standard 0-based UTF-16 line and character offsets.
- Source files use `file:///main/<relpath>` URIs, for example
  `file:///main/main.solc`.
- Incoming worker messages should be JSON-RPC objects. Outgoing worker messages
  are JSON-RPC objects, including responses and
  `textDocument/publishDiagnostics` notifications.

For `monaco-languageclient`, wire a `MessageTransports` pair where the writer
calls `worker.postMessage(message)` and the reader emits each `message` from the
worker's `message` event. The message value should be passed through as the LSP
JSON-RPC object; do not add stdio headers or `Content-Length`.
