---
title: Installation
description: Install iglint as a prebuilt binary, with Cargo, or as a VS Code extension. A PyPI package is planned.
---

iglint is a single executable with no runtime dependencies. Pick whichever channel suits your team; they all install the same binary.

## Prebuilt binaries

Every [GitHub release](https://github.com/Mustry-Solutions/iglint/releases) carries an archive per platform, each with a `.sha256` checksum. The archive `iglint-v0.1.0-<target>.tar.gz` (`.zip` on Windows) holds a directory of the same name with the binary, the README and the licenses:

| Platform | Target | Archive |
|---|---|---|
| Linux x86_64 (static, musl) | `x86_64-unknown-linux-musl` | `.tar.gz` |
| Linux aarch64 (static, musl) | `aarch64-unknown-linux-musl` | `.tar.gz` |
| macOS Intel | `x86_64-apple-darwin` | `.tar.gz` |
| macOS Apple silicon | `aarch64-apple-darwin` | `.tar.gz` |
| Windows x86_64 | `x86_64-pc-windows-msvc` | `.zip` |

The Linux binaries are statically linked and run on any distribution; the Windows binary does not need the Visual C++ runtime. Unpack the archive and put `iglint` (or `iglint.exe`) on your `PATH`. For example, on Linux x86_64:

```sh
curl -sSfLO https://github.com/Mustry-Solutions/iglint/releases/download/v0.1.0/iglint-v0.1.0-x86_64-unknown-linux-musl.tar.gz
curl -sSfLO https://github.com/Mustry-Solutions/iglint/releases/download/v0.1.0/iglint-v0.1.0-x86_64-unknown-linux-musl.tar.gz.sha256
sha256sum -c iglint-v0.1.0-x86_64-unknown-linux-musl.tar.gz.sha256
tar -xzf iglint-v0.1.0-x86_64-unknown-linux-musl.tar.gz
sudo install iglint-v0.1.0-x86_64-unknown-linux-musl/iglint /usr/local/bin/
```

On macOS, use `shasum -a 256 -c` instead of `sha256sum -c`. A binary downloaded with a browser is quarantined by Gatekeeper; `xattr -d com.apple.quarantine iglint` lets it run.

## Cargo

With Rust 1.90 or newer, from the release tag:

```sh
cargo install --locked --git https://github.com/Mustry-Solutions/iglint --tag v0.1.0 iglint
```

Without `--tag`, Cargo builds the latest commit of the `main` branch.

## VS Code extension

The extension is attached to the GitHub releases as VSIX files (it is not on the Visual Studio Marketplace). There is one VSIX per platform with the binary inside, and one without a binary that uses `iglint` from your `PATH`:

```sh
code --install-extension iglint-vscode-v0.1.0-linux-x64.vsix
```

Platform names are `linux-x64`, `linux-arm64`, `darwin-x64`, `darwin-arm64` and `win32-x64`. See [Editors](/guides/editors/) for its settings.

## Python package (pip, uv, pipx; planned)

:::note[Not on PyPI yet]
The Python package is planned but not published yet: `pip install iglint` does not work today. Use a [prebuilt binary](#prebuilt-binaries) or [Cargo](#cargo) until it is. The commands below are the ones that will work once it is on PyPI.
:::

The `iglint` package on PyPI will be a wheel that contains only the binary (no Python code), so it fits into an existing Python tool chain or CI image:

```sh
pip install iglint
# or, as an isolated tool:
uv tool install iglint
pipx install iglint
```

The package will require Python 3.8 or newer to install. Wheels will be built for Linux (x86_64 and aarch64, glibc and musl), macOS (x86_64 and Apple silicon) and Windows (x86_64). On other platforms pip will fall back to the source distribution, which needs a Rust toolchain to build.

Pin a version with `pip install iglint==0.1.0`.

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
