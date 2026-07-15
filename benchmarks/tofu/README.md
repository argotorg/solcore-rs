# tofu `solc-bench` suite

`materialize.py` creates a self-contained Standard JSON suite for the shared
Solcore syntax intersection.  It deliberately measures compilation through
checked Hull generation, rather than Solidity bytecode size or gas metrics.

```sh
python3 benchmarks/tofu/materialize.py /data/$USER/solcore-bench
```

Use the generated directory with tofu's `bench.sbatch` and either the
`solcore-rs` `--standard-json` binary or tofu's `solcore-standard-json` Haskell
adapter.  See the repository-level integration plan for the exact commands.
