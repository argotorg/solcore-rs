This corpus vendors Solcore reference fixtures from Y-Nak/solcore at ac6f8957.

Sources:
- `ok/std`: copied from `/Users/y_nak/github.com/Y-Nak/solcore/std`
- `ok/test/examples` and `fail/test/examples`: copied from `/Users/y_nak/github.com/Y-Nak/solcore/test/examples`

The example split is derived by running the ac6f8957 reference binary in frontend mode (`-n -g`) with the vendored reference std. Files in `ok/test/examples` pass that reference frontend run. Files in `fail/test/examples` are rejected by the reference frontend or hit the recorded 60-second timeout. The full per-file verdict is recorded in `reference-frontend.tsv`. Parser snapshots are kept only for fail fixtures that also produce Rust parser diagnostics.
