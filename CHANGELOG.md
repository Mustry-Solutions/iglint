# Changelog

All notable changes to iglint are listed here. The format follows
[Keep a Changelog](https://keepachangelog.com/en/1.1.0/) and versions follow
[Semantic Versioning](https://semver.org/). Rule codes and names are stable
from 0.1.0 on.

## [Unreleased]

## [0.1.0]

First public release.

### Added

- Linter for Ignition 8.3 projects on disk: Perspective views, Jython 2.7
  scripts (project library, gateway and session events, WebDev, tag events,
  scripts embedded in views), expressions, style classes, page configuration,
  session properties, named queries, resource manifests and gateway tag
  resources.
- More than 100 rules in ten categories (`VIEW`, `NAME`, `BIND`, `SCR`,
  `REF`, `PERF`, `SEC`, `A11Y`, `STYLE`, `TAG`), with cross-resource checks
  over the whole project and its parent projects. Reference in
  [docs/rules](docs/rules/README.md), generated from the rule metadata.
- Jython 2.7 parser, semantic model and expression parser tested against the
  parsers shipped in the Ignition 8.3.3 image; `system.*`, expression
  function and Perspective component facts generated from a real gateway.
- Safe and unsafe auto-fixes (`--fix`, `--unsafe-fixes`, `--diff`) that only
  change the bytes they fix.
- Configuration in `iglint.toml`, `.iglint.toml` or `[tool.iglint]` in
  `pyproject.toml`: rule selection, per-rule severity, options and excludes,
  `[[overrides]]`, `strict`.
- Script suppression comments (`# iglint: ignore[...]`,
  `# iglint: ignore-file[...]`) and a baseline file
  (`iglint baseline`, `--prune-baseline`) that is portable across platforms.
- Output formats: text, concise, grouped, JSON, JSON lines, SARIF, GitHub
  Actions, Azure DevOps, GitLab Code Quality, JUnit and pylint.
- ign-lint 0.6.4 compatibility: `iglint ign-lint` (or the binary called as
  `ign-lint`) with ign-lint's arguments and output, `rule_config.json` and
  whitelist support, an equivalent for every ign-lint rule, and
  `iglint migrate` to convert the configuration.
- Result cache and parallel checking.
- Language server (`iglint server`) and a VS Code extension.
- Distribution as prebuilt binaries, a PyPI wheel (`pip install iglint`),
  pre-commit hooks and VS Code packages.

[Unreleased]: https://github.com/Mustry-Solutions/iglint/compare/v0.1.0...HEAD
[0.1.0]: https://github.com/Mustry-Solutions/iglint/releases/tag/v0.1.0
