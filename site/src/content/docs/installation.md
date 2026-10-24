---
title: Installation
description: Install iglint with pip, uv, pipx or Cargo, as a prebuilt binary, or as a VS Code extension.
---

:::caution[Not published yet]
iglint is preparing its first release, 0.1.0. Until that release is tagged, the PyPI package, the GitHub release downloads and the VS Code packages below do not exist yet, and the GitHub repository may not be public. [Build from source](#from-source) in the meantime. The commands on this page are the ones that will work once 0.1.0 is published.
:::

iglint is a single executable with no runtime dependencies. Pick whichever channel suits your team; they all install the same binary.

## Python package (pip, uv, pipx)

The `iglint` package on PyPI is a wheel that contains only the binary (no Python code), so it fits into an existing Python tool chain or CI image:

```sh
pip install iglint
# or, as an isolated tool:
uv tool install iglint
pipx install iglint
```

The package requires Python 3.8 or newer to install. Wheels are built for Linux (x86_64 and aarch64, glibc and musl), macOS (x86_64 and Apple silicon) and Windows (x86_64). On other platforms pip falls back to the source distribution, which needs a Rust toolchain to build.

To pin a version in CI, use `pip install iglint==0.1.0`.

## Cargo

With Rust 1.90 or newer:

```sh
cargo install --locked --git https://github.com/Mustry-Solutions/iglint iglint
```

## Prebuilt binaries

Every [GitHub release](https://github.com/Mustry-Solutions/iglint/releases) will carry an archive per platform, each with a `.sha256` checksum:

| Platform | Target | Archive |
|---|---|---|
| Linux x86_64 (static, musl) | `x86_64-unknown-linux-musl` | `.tar.gz` |
| Linux aarch64 (static, musl) | `aarch64-unknown-linux-musl` | `.tar.gz` |
| macOS Intel | `x86_64-apple-darwin` | `.tar.gz` |
| macOS Apple silicon | `aarch64-apple-darwin` | `.tar.gz` |
| Windows x86_64 | `x86_64-pc-windows-msvc` | `.zip` |

The Linux binaries are statically linked and run on any distribution; the Windows binary does not need the Visual C++ runtime. Unpack the archive and put `iglint` (or `iglint.exe`) on your `PATH`.

## VS Code extension

The extension is attached to the GitHub releases as VSIX files (it is not on the Visual Studio Marketplace). There is one VSIX per platform with the binary inside, and one without a binary that uses `iglint` from your `PATH`:

```sh
code --install-extension iglint-vscode-v0.1.0-linux-x64.vsix
```

Platform names are `linux-x64`, `linux-arm64`, `darwin-x64`, `darwin-arm64` and `win32-x64`. See [Editors](/guides/editors/) for its settings.

## From source

With a Rust toolchain (the version pinned in `rust-toolchain.toml` is installed automatically by rustup), from a clone of the repository:

```sh
cargo install --locked --path crates/iglint
```

or build without installing:

```sh
cargo build --release -p iglint
./target/release/iglint --version
```

To build the VS Code extension from source, see `editors/vscode` in the repository (Node.js 20 or newer).

## Check the installation

```sh
iglint --version
```

prints the version, the commit and its date, for example `iglint 0.1.0 (5e63b9081 2026-10-24)`.

## Next step

Run it on a small example project in [Getting started](/getting-started/).
