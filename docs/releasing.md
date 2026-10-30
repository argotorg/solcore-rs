# Local releases

All release commands run on a maintainer's computer. No GitHub Actions runner,
CI artifact, or server-side build is used. The validation workflow runs only for
pull requests or manual dispatch, so branch and release-tag pushes do not start
hosted CI.

The pipeline follows a versioned Rust application release: review versions and
notes, commit, run locked builds and tests, package native executables and std,
produce checksums, create an annotated `vX.Y.Z` tag, stage a GitHub draft, inspect
it, then publish. Playground deployment is a separate explicit operation using
the exact archived build. The two services do not have an atomic publish API.

## Distribution decisions

`solcore lsp` bundles the native language server with the compiler. This keeps
compiler, analysis implementation and embedded LSP standard library at one
version. Small editor packages discover the installed compiler independently;
they do not download or embed platform binaries. A compiler update takes effect
after restarting the editor's server. The browser still uses a separate WASM
LSP worker because browser transport and build requirements differ.

The client search order is explicit editor configuration, `SOLCORE_PATH`, the
legacy `SOLCORE_LSP_SERVER` override, Cargo's bin directory (`CARGO_HOME/bin` or
`~/.cargo/bin`), `~/.local/bin`, then absolute entries in `PATH`. Broken explicit
settings are reported instead of silently switching compiler versions. Working
directory and `target/` binaries are not automatically selected. VS Code also
requires workspace trust before starting a native process. Remote editor hosts
(SSH, WSL or containers) need their own matching compiler installation.

| Client | Release artifact | Installation |
| --- | --- | --- |
| VS Code | `solcore-language-<extension-version>.vsix` | `code --install-extension FILE.vsix`; set `solcore.path` if needed |
| Neovim/Vim | `solcore-vim-v<compiler-version>.tar.gz` | Extract `vim-solcore` into the editor's package directory; enable the documented native LSP or vim-lsp client |
| Emacs | `solcore-emacs-v<compiler-version>.tar.gz` | Put `emacs-solcore` on `load-path`, load the mode and enable Eglot or lsp-mode |

See [editor setup](../editors/README.md) for exact configuration. VSIX versions
are independent of compiler versions. This release does not publish to VS Code
Marketplace, Open VSX, MELPA or a package registry; those require separate
publisher accounts and release policies. The installable GitHub artifacts work
without them.

The initial release is an executable distribution, not a crates.io library
release. Current crates depend on Git-pinned Sonatina and source files in the
repository's `std/`; crates.io packages cannot use Git dependencies and cannot
include files outside their package. `publish = false` is inherited throughout
the workspace. Supporting crates.io later requires restructuring those inputs
and assigning registry versions to internal dependencies first. Cargo's
[dependency documentation](https://doc.rust-lang.org/cargo/reference/specifying-dependencies.html)
describes these registry restrictions.

The small Python orchestrator uses Cargo, npm, GitHub CLI and Wrangler directly.
It deliberately does not generate a cargo-dist/release-plz CI workflow: native
builds happen on available local hosts, and Cloudflare's archived web build is
part of the same release bundle.

## Tools and credentials

- Python 3.12+, Git, and the Rust toolchain selected by `rust-toolchain.toml`.
- Node.js 22+ and npm. `npm ci` consumes the committed lockfiles; Wrangler,
  Binaryen and VSIX packaging tools are pinned there.
- `wasm-pack` 0.15.0 and the Rust `wasm32-unknown-unknown` target for playground.
- GitHub CLI (`gh`) authenticated for the explicitly chosen repository when
  staging/publishing. Local preparation needs no GitHub write credentials.
- Cloudflare credentials only for production deployment: log in using the
  playground's pinned Wrangler (`npm exec -- wrangler login`), or set
  `CLOUDFLARE_API_TOKEN` and `CLOUDFLARE_ACCOUNT_ID` for the intended account.
  The existing Worker name and managed custom domain stay in `wrangler.jsonc`.
  Do not put credentials in the release bundle.
- Optional external backend checks (`--e2e`): `solc`, `anvil`, and `cast` on
  `PATH`, with versions supporting the fixtures' Osaka EVM rules. The previous
  validation configuration used solc 0.8.35 and Foundry 1.7.1.

```sh
rustup target add wasm32-unknown-unknown
cargo install wasm-pack --version 0.15.0 --locked
python3 scripts/release.py plan
```

Run `python` instead of `python3` where that is the Python 3.12 command (for
example on Windows). A native linker/C toolchain appropriate to the host is
required. macOS archives are currently unsigned; no signing or notarization
credentials are assumed.

## 1. Prepare the release commit

The first release is the prerelease `0.1.0-alpha`. To change the compiler and
playground version, for example to another prerelease such as `0.1.0-rc.1`:

```sh
python3 scripts/release.py set-version 0.1.0-alpha
```

This updates the shared Rust workspace version, Cargo.lock and playground npm
versions. It uses Cargo's cached dependency metadata (`--offline`); run
`cargo fetch --locked` first on a fresh checkout. It does not commit or tag.
Edit `releases/v0.1.0-alpha.md` (create the corresponding file for any new
version). If the editor client changed, independently bump the VSIX's
`package.json` and lockfile version with, for example,
`npm version patch --no-git-tag-version` in its directory. Keep the `Version:`
header of `editors/emacs-solcore/solcore-mode.el` equal to the compiler version,
which also names the Vim and Emacs archives, and update the version in the
installation commands of `README.md` and the editor package READMEs.

Review and commit all release sources, scripts, lockfiles and notes. `prepare`
requires tracked changes to be committed and builds a `git archive` snapshot of
HEAD. Untracked local files, `.env` files, cached WASM and old build outputs are
excluded. Newly created source files must be added to the release commit.

## 2. Build and test locally

On the primary machine (native binaries, playground and all editor packages):

```sh
python3 scripts/release.py prepare --output target/releases/primary
```

The command runs formatting, Clippy, workspace tests including native LSP
protocol tests, and documentation checks. It builds native release executables
with `--locked` and tests the actual package layout outside the checkout. It
also installs locked npm dependencies, tests and builds both WASM workers,
runs browser execution tests, validates a Cloudflare deployment with
`--dry-run`, tests the VS Code client and builds a VSIX. No upload is performed.
`check` runs the Rust gates directly on the current working tree while developing.

Use `prepare --e2e ...` or `check --e2e` to additionally require the external
solc/Foundry suites. The manifest records whether these optional external tests
ran. Regular workspace tests and browser WASM execution tests always run when
their corresponding components are prepared. Performance guards can be run
with `scripts/check-compile-performance.sh` before the release; they are not
part of the portable host build command.

Each native archive includes `solcore`, `solcore-fmt`, the compatibility commands
`solcore-driver`/`solcore-lsp`, adjacent `std/`, license, README and provenance.
Unix hosts produce `.tar.gz`; Windows produces `.zip`. The host triple is in
the filename. Supported build hosts are macOS (Apple Silicon/Intel), Linux
(x86_64/aarch64 GNU), and Windows (x86_64/aarch64 MSVC). A host is **available in
a release only after that host successfully builds and tests its artifact**.
This script does not claim cross-platform validation from a macOS build.
Windows packaging is implemented, but Windows is not yet release-qualified:
some existing type-checker/specialization fixtures still assume Unix virtual
paths. The full test gate must pass on Windows before distributing its binaries;
do not list Windows downloads in the initial release without that work. WSL can
use a corresponding Linux archive.
GNU Linux binaries inherit their build host's glibc minimum; use the oldest
Linux environment you intend to support and state that baseline in the notes.

On each additional local machine, check out the **same release commit** and run:

```sh
python3 scripts/release.py prepare --components native --output target/releases/linux-x64
```

Transfer the complete bundle directories back to the primary computer. Build
playground/editor components only once; their names are platform independent.
You can also build them separately with `--components playground editors`.
Combine the directories:

```sh
python3 scripts/release.py collect --output target/releases/v0.1.0-alpha \
  target/releases/primary target/releases/linux-x64
python3 scripts/release.py verify --bundle target/releases/v0.1.0-alpha
```

For a single-host release, use the primary directory directly. Outputs must be
new directories to prevent stale files from entering a release. Failed builds
do not expose a partially complete bundle. Collection rejects mismatched
versions, commits, checksums and conflicting duplicate asset names. Source
archives use Git's modes and line endings so they match across hosts.

`manifest.json` inventories assets, source commit and version. `SHA256SUMS`
covers those assets and the manifest. Checksums detect corruption; they are not
a signing or third-party attestation system. Toolchain pinning and normalized
archive metadata reduce drift, but byte-for-byte native reproducibility across
different machines is not promised.

## 3. Stage and publish GitHub binaries

First push the reviewed release commit to the intended repository using your
normal review/merge process. The script requires that exact commit to exist
there and requires the local checkout to match the bundle. It never infers the
repository from `origin` (which may be a personal fork).

```sh
python3 scripts/release.py stage-github \
  --repo argotorg/solcore-rs --bundle target/releases/v0.1.0-alpha
```

This creates an annotated version tag through GitHub's authenticated Git API at
the recorded commit, then creates a **draft** release with the committed notes
and uploads all packages/checksums. An existing tag must resolve to the same
commit. Native, playground, source and all three editor packages must be
present. Uploads are downloaded again and checked against local hashes.

Inspect the draft's notes, platform coverage and downloads. Then:

```sh
python3 scripts/release.py publish-github \
  --repo argotorg/solcore-rs --bundle target/releases/v0.1.0-alpha
```

This repeats tag and remote-asset verification before making the draft public.
Prerelease versions are marked as prereleases and never marked Latest.
See GitHub CLI's [release creation](https://cli.github.com/manual/gh_release_create)
and [asset upload](https://cli.github.com/manual/gh_release_upload) documentation.

A failed upload leaves a draft. Re-running `stage-github` may replace draft
assets; it never overwrites a public release or moves a tag. If the draft has
extra stale assets, remove those explicitly before retrying. Never reuse an
already published version for different bytes. Fetch the created tag with
`git fetch <your-release-remote> --tags` if you need it locally.

## 4. Publish the playground

First validate the independently deployable, archived web package:

```sh
python3 scripts/release.py deploy-playground --bundle target/releases/v0.1.0-alpha
```

This extracts the verified archive into a temporary directory, installs its
locked tooling, and runs Wrangler's deployment dry run. It does not rebuild
Rust, WASM or frontend assets, and does not read untracked checkout `.env` files.
When ready to update the production Worker:

```sh
python3 scripts/release.py deploy-playground \
  --bundle target/releases/v0.1.0-alpha --execute
```

The same dry run happens before the production deploy. The existing Worker
`solcore-playground` and custom domain remain managed by the infrastructure team;
the script does not create domains or enable workers.dev/preview URLs. Verify
the deployed `/release.json` and try Compile/Run plus editor hover after deploy.
Cloudflare's [Wrangler deploy documentation](https://developers.cloudflare.com/workers/wrangler/commands/workers/#deploy)
describes the local upload and dry-run behavior.

If Cloudflare fails after GitHub publication, retry only `deploy-playground` with
the same verified bundle. To roll back the website, deploy a retained older
bundle. Keep the GitHub release immutable; publish a new compiler version to
correct its binaries. The bundle's version and commit remain visible at
`/release.json`, making a temporary difference between GitHub and the website
observable.
