#!/usr/bin/env python3
"""Local release pipeline. Python 3.12+, no hosted CI or Python dependencies."""

from __future__ import annotations

import argparse
import gzip
import hashlib
import json
import os
from pathlib import Path
import re
import shlex
import shutil
import subprocess
import sys
import tarfile
import tempfile
import tomllib
import zipfile

ROOT = Path(__file__).resolve().parents[1]
COMPONENTS = ("native", "playground", "editors")
TARGETS = (
    "aarch64-apple-darwin", "x86_64-apple-darwin",
    "x86_64-unknown-linux-gnu", "aarch64-unknown-linux-gnu",
    "x86_64-pc-windows-msvc", "aarch64-pc-windows-msvc",
)
SEMVER = re.compile(r"(0|[1-9][0-9]*)\.(0|[1-9][0-9]*)\.(0|[1-9][0-9]*)(?:-([0-9A-Za-z-]+(?:\.[0-9A-Za-z-]+)*))?")


class ReleaseError(Exception):
    pass


def require(condition, message):
    if not condition:
        raise ReleaseError(message)


def run(*args, cwd=None, capture=False, env=None):
    command = [str(arg) for arg in args]
    # npm is a .cmd launcher on Windows; resolve it explicitly for subprocess.
    if os.name == "nt" and command[0] == "npm":
        command[0] = shutil.which("npm.cmd") or "npm.cmd"
    print("+ " + shlex.join(command), flush=True)
    result = subprocess.run(command, cwd=cwd or ROOT, check=True, text=True,
                            stdout=subprocess.PIPE if capture else None, env=env)
    return result.stdout.strip() if capture else ""


def read_json(path):
    return json.loads(Path(path).read_text(encoding="utf-8"))


def write_json(path, value):
    Path(path).write_text(json.dumps(value, indent=2, sort_keys=True) + "\n", encoding="utf-8")


def digest(path):
    with Path(path).open("rb") as stream:
        return hashlib.file_digest(stream, "sha256").hexdigest()


def deployed_files(cwd):
    files = [cwd / "wrangler.jsonc", *(cwd / "dist").rglob("*")]
    require(all(not path.is_symlink() for path in files), "Linked deployment input")
    return {path.relative_to(cwd).as_posix(): digest(path) for path in files if path.is_file()}


def node_version(root, env):
    value = run("node", "--version", cwd=root, env=env, capture=True)
    require(int(value.lstrip("v").split(".")[0]) >= 22, "Release tooling requires Node.js 22 or newer on PATH")
    return value


def version(root=None):
    root = root or ROOT
    with (root / "Cargo.toml").open("rb") as stream:
        value = tomllib.load(stream)["workspace"]["package"]["version"]
    validate_version(value)
    return value


def validate_version(value):
    match = SEMVER.fullmatch(value)
    require(match is not None, "Use SemVer MAJOR.MINOR.PATCH[-prerelease], without v or build metadata")
    if match[4]:
        require(all(not part.isdigit() or part == "0" or not part.startswith("0")
                    for part in match[4].split(".")), "Numeric prerelease identifiers cannot have leading zeroes")


def check_versions(root):
    value = version(root)
    for name in ("package.json", "package-lock.json"):
        data = read_json(root / "playground" / name)
        require(data["version"] == value, f"playground/{name} version differs from Rust; run set-version")
        if name == "package-lock.json":
            require(data["packages"][""]["version"] == value, "Playground lockfile root version differs")
    for manifest in (root / "crates").glob("*/Cargo.toml"):
        with manifest.open("rb") as stream:
            package = tomllib.load(stream)["package"]
        require(package["version"] == {"workspace": True}, f"{manifest}: inherit workspace version")
    return value


def set_version(value):
    validate_version(value)
    old = version()
    manifest = ROOT / "Cargo.toml"
    text = manifest.read_text(encoding="utf-8")
    text = text.replace(f'[workspace.package]\nversion = "{old}"',
                        f'[workspace.package]\nversion = "{value}"', 1)
    manifest.write_text(text, encoding="utf-8")
    for name in ("package.json", "package-lock.json"):
        path = ROOT / "playground" / name
        data = read_json(path)
        data["version"] = value
        if name == "package-lock.json":
            data["packages"][""]["version"] = value
            for key in ("../crates/lsp/pkg", "../crates/wasm/pkg"):
                if key in data["packages"]:
                    data["packages"][key]["version"] = value
        # Keep npm's existing ordering and two-space formatting.
        path.write_text(json.dumps(data, indent=2) + "\n", encoding="utf-8")
    run("cargo", "metadata", "--offline", "--format-version", "1", capture=True)
    print(f"Updated compiler and playground to {value}. Review Cargo.lock and add release notes before committing.")


def clean_commit():
    require(not run("git", "status", "--porcelain", "--untracked-files=no", capture=True),
            "Commit tracked changes before preparing/publishing. Untracked files are excluded from the source snapshot.")
    return run("git", "rev-parse", "HEAD", capture=True)


def rust_checks(root, e2e=False, env=None):
    check_versions(root)
    run(sys.executable, "-m", "unittest", "discover", "-s", "scripts/tests", cwd=root, env=env)
    run("cargo", "fmt", "--all", "--", "--check", cwd=root, env=env)
    run("cargo", "clippy", "--workspace", "--all-targets", "--all-features", "--locked",
        "--", "-D", "warnings", cwd=root, env=env)
    run("cargo", "test", "--workspace", "--all-features", "--locked", cwd=root, env=env)
    doc_env = dict(env or os.environ, RUSTDOCFLAGS="-D warnings")
    run("cargo", "doc", "--workspace", "--all-features", "--no-deps", "--locked", cwd=root, env=doc_env)
    if e2e:
        for tool in ("solc", "anvil", "cast"):
            require(shutil.which(tool), f"Install {tool} for --e2e")
        e2e_env = dict(env or os.environ, E2E="1", E2E_REQUIRED="1", ANVIL_HARDFORK="osaka")
        run("cargo", "test", "--profile", "e2e", "-p", "solcore-yul", "--test", "snapshots", "--locked",
            "solc_strict_assembly_compiles_snapshots_and_repros_when_present", cwd=root, env=e2e_env)
        run("cargo", "test", "--profile", "e2e", "-p", "solcore-yul", "-p", "solcore-sonatina",
            "--test", "e2e", "--locked", "--", "--test-threads=1", cwd=root, env=e2e_env)
        run("cargo", "test", "--profile", "e2e", "-p", "solcore-wasm", "--locked", cwd=root,
            env=dict(e2e_env, SOLCORE_WASM_TEST_BACKEND="solc"))


def pack_tree(source, destination, top, epoch):
    """Stable archive metadata; release binaries themselves need not be bit reproducible."""
    files = sorted(path for path in source.rglob("*") if path.is_file())
    require(all(not path.is_symlink() for path in source.rglob("*")), "Refusing symlink in release payload")
    if destination.suffix == ".zip":
        with zipfile.ZipFile(destination, "w", zipfile.ZIP_DEFLATED) as archive:
            for path in files:
                info = zipfile.ZipInfo(f"{top}/{path.relative_to(source).as_posix()}")
                info.external_attr = (0o100755 if os.access(path, os.X_OK) else 0o100644) << 16
                archive.writestr(info, path.read_bytes(), compress_type=zipfile.ZIP_DEFLATED)
    else:
        with destination.open("wb") as raw, gzip.GzipFile(fileobj=raw, mode="wb", filename="", mtime=epoch) as gz:
            with tarfile.open(fileobj=gz, mode="w") as archive:
                for path in files:
                    info = archive.gettarinfo(str(path), f"{top}/{path.relative_to(source).as_posix()}")
                    info.uid = info.gid = 0
                    info.uname = info.gname = ""
                    info.mtime = epoch
                    with path.open("rb") as stream:
                        archive.addfile(info, stream)


def unpack(archive, destination):
    # All archives produced/consumed by deployment are tarballs. Python's data
    # filter rejects absolute paths, escaping links and device entries.
    with tarfile.open(archive, "r:*") as stream:
        stream.extractall(destination, filter="data")


def snapshot(commit, destination):
    archive = destination.parent / "source.tar"
    run("git", "archive", "--format=tar", "--output", archive, commit)
    destination.mkdir()
    unpack(archive, destination)
    archive.unlink()


def pack_source(commit, destination, top, epoch):
    # Git, rather than the host filesystem, supplies file modes and line endings.
    # Consequently native bundles from Windows/macOS/Linux share this asset.
    with tempfile.TemporaryDirectory(prefix="solcore-source-") as tmp:
        archive = Path(tmp) / "source.tar"
        run("git", "-c", "tar.umask=0002", "archive", "--format=tar", "--prefix", top + "/", "--output", archive, commit)
        with archive.open("rb") as source, destination.open("wb") as raw:
            with gzip.GzipFile(fileobj=raw, mode="wb", filename="", mtime=epoch) as gz:
                shutil.copyfileobj(source, gz)


def build_native(root, output, metadata, epoch, env, e2e):
    rust_info = run("rustc", "-vV", cwd=root, capture=True, env=env)
    target = next(line.removeprefix("host: ") for line in rust_info.splitlines() if line.startswith("host: "))
    require(target in TARGETS, f"Unsupported release host {target}; supported hosts: {', '.join(TARGETS)}")
    rust_checks(root, e2e, env)
    run("cargo", "build", "--release", "--locked", "--target", target,
        "-p", "solcore-driver", "-p", "solcore-lsp", "--features", "solcore-lsp/native", "-p", "solcore-fmt",
        cwd=root, env=env)
    name = f"solcore-v{metadata['version']}-{target}"
    payload = root.parent / name
    payload.mkdir()
    suffix = ".exe" if "windows" in target else ""
    for binary in ("solcore", "solcore-driver", "solcore-lsp", "solcore-fmt"):
        shutil.copy2(Path(env["CARGO_TARGET_DIR"]) / target / "release" / (binary + suffix), payload)
    shutil.copytree(root / "std", payload / "std")
    shutil.copy2(root / "LICENSE", payload)
    shutil.copy2(root / "README.md", payload)
    info = dict(metadata, target=target, rustc=rust_info, e2e=e2e)
    write_json(payload / "release.json", info)
    # Run from outside the checkout, against the actual archive layout.
    smoke = root.parent / "smoke"
    smoke.mkdir()
    (smoke / "main.sol").write_text("pragma solcore;\nimport * from std;\nfunction main() returns (word) { return 42; }\n",
                                  encoding="utf-8")
    smoke_env = dict(env)
    smoke_env.pop("SOLCORE_STD", None)
    require(run(payload / ("solcore" + suffix), "--version", cwd=smoke, capture=True, env=smoke_env)
            == f"solcore {metadata['version']}", "Compiler version mismatch")
    run(payload / ("solcore" + suffix), smoke / "main.sol", "--emit-yul", cwd=smoke, capture=True, env=smoke_env)
    run(payload / ("solcore-fmt" + suffix), "--version", cwd=smoke, env=smoke_env)
    run(payload / ("solcore" + suffix), "lsp", "--version", cwd=smoke, env=smoke_env)
    archive = output / (name + (".zip" if suffix else ".tar.gz"))
    pack_tree(payload, archive, name, epoch)
    return [(archive, "native", info)]


def build_playground(root, output, metadata, epoch, env):
    cwd = root / "playground"
    env = dict(env, VITE_BASE="/")
    node = node_version(cwd, env)
    wasm_pack = run("wasm-pack", "--version", cwd=cwd, capture=True, env=env)
    require(wasm_pack == "wasm-pack 0.15.0", "Install the pinned wasm-pack: cargo install wasm-pack --version 0.15.0 --locked")
    run("npm", "ci", "--include=dev", "--ignore-scripts", cwd=cwd, env=env)
    run("npm", "run", "test:unit", cwd=cwd, env=env)
    run("npm", "run", "build", cwd=cwd, env=env)
    run("npm", "run", "test:wasm", cwd=cwd, env=env)
    write_json(cwd / "dist" / "release.json", metadata)
    run("npm", "run", "deploy:check", cwd=cwd, env=env)
    name = f"solcore-playground-v{metadata['version']}"
    payload = root.parent / name
    payload.mkdir()
    shutil.copytree(cwd / "dist", payload / "dist")
    for filename in ("package.json", "package-lock.json", "wrangler.jsonc", ".npmrc"):
        shutil.copy2(cwd / filename, payload)
    shutil.copy2(root / "LICENSE", payload)
    archive = output / (name + ".tar.gz")
    pack_tree(payload, archive, name, epoch)
    return [(archive, "playground", dict(metadata, node=node, wasm_pack=wasm_pack))]


def build_editors(root, output, metadata, epoch, env):
    cwd = root / "editors" / "vscode-solcore"
    node_version(cwd, env)
    run("npm", "ci", "--include=dev", "--ignore-scripts", cwd=cwd, env=env)
    run("npm", "run", "test:unit", cwd=cwd, env=env)
    extension_version = read_json(cwd / "package.json")["version"]
    archive = output / f"solcore-language-{extension_version}.vsix"
    run("npm", "run", "package", "--", "--out", archive, cwd=cwd, env=env)
    result = [(archive, "vscode", dict(metadata, extension_version=extension_version))]
    for editor in ("vim", "emacs"):
        name = f"solcore-{editor}-v{metadata['version']}"
        archive = output / (name + ".tar.gz")
        pack_tree(root / "editors" / f"{editor}-solcore", archive, f"{editor}-solcore", epoch)
        result.append((archive, editor, metadata))
    return result


def seal(output, metadata, records):
    assets = {}
    for path, kind, provenance in records:
        require(path.name not in assets, f"Duplicate asset {path.name}")
        assets[path.name] = dict(kind=kind, sha256=digest(path), size=path.stat().st_size, provenance=provenance)
    manifest = dict(metadata, schema=1, assets=assets)
    write_json(output / "manifest.json", manifest)
    sums = {name: info["sha256"] for name, info in assets.items()}
    sums["manifest.json"] = digest(output / "manifest.json")
    (output / "SHA256SUMS").write_text("".join(f"{value}  {name}\n" for name, value in sorted(sums.items())), encoding="utf-8")
    verify_bundle(output)


def verify_bundle(output):
    output = Path(output)
    manifest = read_json(output / "manifest.json")
    require(manifest.get("schema") == 1, "Unsupported bundle schema")
    validate_version(manifest["version"])
    require(re.fullmatch(r"[0-9a-f]{40}", manifest["commit"]), "Invalid source commit")
    require(manifest["assets"], "Empty release bundle")
    sums = {}
    for line in (output / "SHA256SUMS").read_text(encoding="utf-8").splitlines():
        value, name = line.split("  ", 1)
        require(name not in sums, "Duplicate checksum entry")
        sums[name] = value
    expected = {"manifest.json": digest(output / "manifest.json")}
    for name, info in manifest["assets"].items():
        require(re.fullmatch(r"[A-Za-z0-9][A-Za-z0-9._-]*", name) and name not in ("manifest.json", "SHA256SUMS"),
                f"Invalid asset name {name}")
        path = output / name
        require(path.is_file() and not path.is_symlink(), f"Missing or linked asset {name}")
        require(info["provenance"]["version"] == manifest["version"] and
                info["provenance"]["commit"] == manifest["commit"], f"Mixed versions/commits: {name}")
        require(path.stat().st_size == info["size"] and digest(path) == info["sha256"], f"Corrupt asset {name}")
        expected[name] = info["sha256"]
    require(sums == expected, "SHA256SUMS does not match the exact release manifest")
    return manifest


def new_output(path):
    path = Path(path).resolve()
    require(not path.exists(), f"Output already exists: {path}; choose a new directory to avoid stale artifacts")
    path.parent.mkdir(parents=True, exist_ok=True)
    return path


def prepare(args):
    commit = clean_commit()
    value = check_versions(ROOT)
    metadata = dict(version=value, commit=commit)
    epoch = int(run("git", "show", "-s", "--format=%ct", commit, capture=True))
    destination = new_output(args.output)
    # Build only committed sources; unrelated local files, .env files, and old
    # wasm output cannot leak into releases. Keep output invisible until sealed.
    with tempfile.TemporaryDirectory(prefix="solcore-release-", dir=destination.parent) as tmp:
        work = Path(tmp)
        root = work / "source"
        snapshot(commit, root)
        output = work / "bundle"
        output.mkdir()
        check_versions(root)
        notes = root / "releases" / f"v{value}.md"
        require(notes.is_file(), f"Write and commit releases/v{value}.md before preparing")
        shutil.copy2(notes, output / "release-notes.md")
        source_archive = output / f"solcore-v{value}-source.tar.gz"
        pack_source(commit, source_archive, f"solcore-v{value}", epoch)
        records = [(source_archive, "source", metadata), (output / "release-notes.md", "notes", metadata)]
        env = dict(os.environ, CARGO_TARGET_DIR=str(work / "target"), SOURCE_DATE_EPOCH=str(epoch),
                   WRANGLER_SEND_METRICS="false", CI="true")
        # CI=true only makes local tools noninteractive; no CI service is used.
        for component in args.components:
            if component == "native":
                records += build_native(root, output, metadata, epoch, env, args.e2e)
            elif component == "playground":
                records += build_playground(root, output, metadata, epoch, env)
            else:
                records += build_editors(root, output, metadata, epoch, env)
        seal(output, metadata, records)
        shutil.move(str(output), destination)
    print(f"Prepared and verified {destination}")


def collect(args):
    manifests = [(Path(path).resolve(), verify_bundle(path)) for path in args.bundles]
    first = manifests[0][1]
    metadata = {key: first[key] for key in ("version", "commit")}
    destination = new_output(args.output)
    with tempfile.TemporaryDirectory(prefix="solcore-collect-", dir=destination.parent) as tmp:
        output = Path(tmp) / "bundle"
        output.mkdir()
        records = {}
        for source, manifest in manifests:
            require(all(manifest[key] == value for key, value in metadata.items()), "Cannot combine different versions/commits")
            for name, info in manifest["assets"].items():
                if name in records:
                    require(digest(output / name) == info["sha256"], f"Different builds claim the same asset: {name}")
                    continue
                shutil.copy2(source / name, output / name)
                records[name] = (output / name, info["kind"], info["provenance"])
        seal(output, metadata, records.values())
        shutil.move(str(output), destination)
    print(f"Collected and verified {destination}")


def github_context(args):
    manifest = verify_bundle(args.bundle)
    require(re.fullmatch(r"[A-Za-z0-9_.-]+/[A-Za-z0-9_.-]+", args.repo), "Use explicit GitHub OWNER/REPO")
    require(clean_commit() == manifest["commit"], "Check out the bundle's source commit before publishing")
    require(check_versions(ROOT) == manifest["version"], "Source and bundle versions differ")
    kinds = {info["kind"] for info in manifest["assets"].values()}
    require({"native", "playground", "vscode", "vim", "emacs", "notes", "source"} <= kinds,
            "Collect native, playground and all editor packages before staging a complete release")
    run("gh", "auth", "status", "--hostname", "github.com")
    remote_commit = run("gh", "api", f"repos/{args.repo}/commits/{manifest['commit']}", "--jq", ".sha", capture=True)
    require(remote_commit == manifest["commit"], "Push the reviewed release commit to the selected repository first")
    return manifest, f"v{manifest['version']}"


def remote_tag(repo, tag):
    refs = json.loads(run("gh", "api", f"repos/{repo}/git/matching-refs/tags/{tag}", capture=True))
    return next((ref for ref in refs if ref["ref"] == f"refs/tags/{tag}"), None)


def check_remote_tag(repo, tag, commit):
    require(remote_tag(repo, tag) is not None, f"Missing remote tag {tag}")
    resolved = run("gh", "api", f"repos/{repo}/commits/{tag}", "--jq", ".sha", capture=True)
    require(resolved == commit, f"Remote {tag} points to another commit; never move a release tag")


def remote_release(repo, tag):
    releases = json.loads(run("gh", "release", "list", "--repo", repo, "--limit", "1000",
                             "--json", "tagName,isDraft", capture=True))
    return next((release for release in releases if release["tagName"] == tag), None)


def stage_github(args):
    manifest, tag = github_context(args)
    existing = remote_release(args.repo, tag)
    require(existing is None or existing["isDraft"], "Release already public; published assets are immutable")
    if remote_tag(args.repo, tag) is None:
        # Create an annotated tag at the reviewed commit using the authenticated
        # GitHub API. No branch push, force push, or implicit default branch.
        tag_sha = run("gh", "api", "--method", "POST", f"repos/{args.repo}/git/tags",
                      "-f", f"tag={tag}", "-f", f"message=Solcore {tag}",
                      "-f", f"object={manifest['commit']}", "-f", "type=commit", "--jq", ".sha", capture=True)
        run("gh", "api", "--method", "POST", f"repos/{args.repo}/git/refs",
            "-f", f"ref=refs/tags/{tag}", "-f", f"sha={tag_sha}")
    check_remote_tag(args.repo, tag, manifest["commit"])
    if existing is None:
        command = ["gh", "release", "create", tag, "--repo", args.repo, "--verify-tag", "--draft",
                   "--title", f"Solcore {tag}", "--notes-file", Path(args.bundle) / "release-notes.md"]
        if "-" in manifest["version"]:
            command.append("--prerelease")
        run(*command)
    else:
        run("gh", "release", "edit", tag, "--repo", args.repo, "--title", f"Solcore {tag}",
            "--notes-file", Path(args.bundle) / "release-notes.md")
    assets = [Path(args.bundle) / name for name in sorted(manifest["assets"])]
    assets += [Path(args.bundle) / name for name in ("manifest.json", "SHA256SUMS")]
    run("gh", "release", "upload", tag, *assets, "--repo", args.repo, "--clobber")
    verify_remote(args.repo, tag, args.bundle, manifest)
    print(f"Draft ready for review: https://github.com/{args.repo}/releases/tag/{tag}")


def verify_remote(repo, tag, bundle, manifest):
    expected = set(manifest["assets"]) | {"manifest.json", "SHA256SUMS"}
    release = json.loads(run("gh", "release", "view", tag, "--repo", repo, "--json", "assets,isDraft", capture=True))
    require(release["isDraft"], "Expected a draft release")
    require({asset["name"] for asset in release["assets"]} == expected,
            "Remote draft assets differ; remove stale assets explicitly before retrying")
    with tempfile.TemporaryDirectory(prefix="solcore-download-") as tmp:
        run("gh", "release", "download", tag, "--repo", repo, "--dir", tmp)
        for name in expected:
            require(digest(Path(tmp) / name) == digest(Path(bundle) / name), f"Remote checksum differs: {name}")
        verify_bundle(tmp)


def publish_github(args):
    manifest, tag = github_context(args)
    check_remote_tag(args.repo, tag, manifest["commit"])
    verify_remote(args.repo, tag, args.bundle, manifest)
    run("gh", "release", "edit", tag, "--repo", args.repo, "--draft=false",
        "--prerelease=true" if "-" in manifest["version"] else "--prerelease=false",
        "--latest=false" if "-" in manifest["version"] else "--latest")


def deploy_playground(args):
    manifest = verify_bundle(args.bundle)
    assets = [name for name, info in manifest["assets"].items() if info["kind"] == "playground"]
    require(len(assets) == 1, "Expected exactly one playground archive")
    with tempfile.TemporaryDirectory(prefix="solcore-deploy-") as tmp:
        root = Path(tmp)
        unpack(Path(args.bundle) / assets[0], root)
        cwd = root / f"solcore-playground-v{manifest['version']}"
        require(read_json(cwd / "dist" / "release.json") == {key: manifest[key] for key in ("version", "commit")},
                "Playground provenance differs from release manifest")
        # Install only the locked tooling and deploy the archived dist. Never
        # rebuild at publication time or read the current checkout's .env files.
        env = dict(os.environ, WRANGLER_SEND_METRICS="false", CI="true")
        node_version(cwd, env)
        before = deployed_files(cwd)
        run("npm", "ci", "--include=dev", "--ignore-scripts", cwd=cwd, env=env)
        run("npm", "run", "deploy:check", cwd=cwd, env=env)
        require(deployed_files(cwd) == before, "Deployment inputs changed during validation; refusing publication")
        if args.execute:
            run("npm", "run", "deploy", cwd=cwd, env=env)
        else:
            print("Deployment validated locally. Add --execute to publish these exact assets to Cloudflare.")


def main():
    require(sys.version_info >= (3, 12), "Use Python 3.12 or newer")
    parser = argparse.ArgumentParser(description=__doc__)
    commands = parser.add_subparsers(dest="command", required=True)
    commands.add_parser("plan", help="Show version and distribution policy without changing anything")
    bump = commands.add_parser("set-version", help="Update Rust/playground versions and Cargo.lock; does not commit")
    bump.add_argument("version")
    check = commands.add_parser("check", help="Run local Rust quality gates")
    check.add_argument("--e2e", action="store_true", help="Also run external solc/Foundry tests")
    build = commands.add_parser("prepare", help="Build/test committed sources into a new local bundle")
    build.add_argument("--output", required=True)
    build.add_argument("--components", nargs="+", choices=COMPONENTS, default=list(COMPONENTS))
    build.add_argument("--e2e", action="store_true")
    merge = commands.add_parser("collect", help="Merge bundles built on different local hosts at the same commit")
    merge.add_argument("--output", required=True)
    merge.add_argument("bundles", nargs="+")
    verify = commands.add_parser("verify", help="Verify local manifest, provenance and checksums")
    verify.add_argument("--bundle", required=True)
    for command in ("stage-github", "publish-github"):
        publish = commands.add_parser(command, help="Write to GitHub (explicit repository required)")
        publish.add_argument("--repo", required=True, help="OWNER/REPO; never inferred from origin")
        publish.add_argument("--bundle", required=True)
    deploy = commands.add_parser("deploy-playground", help="Dry-run archived Cloudflare deployment; --execute publishes")
    deploy.add_argument("--bundle", required=True)
    deploy.add_argument("--execute", action="store_true")
    args = parser.parse_args()
    if args.command == "plan":
        print(f"Solcore v{check_versions(ROOT)}; components: {', '.join(COMPONENTS)}")
        print("Native builds run and test on each host; collect their bundles before GitHub staging.")
        print("GitHub draft staging, GitHub publication and Cloudflare deployment are explicit local commands.")
    elif args.command == "set-version":
        set_version(args.version)
    elif args.command == "check":
        rust_checks(ROOT, args.e2e)
    elif args.command == "verify":
        manifest = verify_bundle(args.bundle)
        print(f"Verified v{manifest['version']} ({manifest['commit']})")
    else:
        {"prepare": prepare, "collect": collect, "stage-github": stage_github,
         "publish-github": publish_github, "deploy-playground": deploy_playground}[args.command](args)


if __name__ == "__main__":
    try:
        main()
    except (ReleaseError, subprocess.CalledProcessError, OSError, ValueError, KeyError, tarfile.TarError) as error:
        print(f"release: {error}", file=sys.stderr)
        sys.exit(1)
