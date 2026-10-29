# Solcore frontend regression corpus

This corpus contains 499 historical examples under `test/examples/`, preserving
their module layout while using the canonical `.sol` syntax. Parseable sources
are formatted with `solcore-fmt`; intentional syntax-error fixtures remain
unchanged. The `ok/test/examples/` and `fail/test/examples/` directories retain
their historical classifications rather than defining current language behavior.

The corpus tests load the workspace standard library directly from
[`std/`](../../../../../std/). Dedicated parser and frontend tests also check
each standard-library source as an entry point. The `test/imports/` and
`known-diagnostic-gaps/` trees contain additional
Rust compiler regressions outside the historical example set.

The Rust compiler and its tests define current language behavior.

Add new regressions to dedicated test/fixture trees rather than the historical
example trees. Parser snapshots are retained only for historically failing
sources that also exercise a Rust parser diagnostic.
