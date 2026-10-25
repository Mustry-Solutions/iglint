# Contributing to iglint

Thanks for helping. Read [DESIGN.md](DESIGN.md) first: it is the contract
between the crates, and its principles (no false positives, exact Jython,
byte-exact fixes, precise locations, no customer data) decide most reviews.

## Setup

* Rust: the toolchain pinned in `rust-toolchain.toml` is installed
  automatically by rustup. The minimum supported version is `rust-version` in
  `Cargo.toml` (checked in CI).
* Node.js 20+ for the VS Code extension (`editors/vscode`).
* Docker and the `inductiveautomation/ignition:8.3.3` image only to record new
  oracle outputs or regenerate the stubs (see below). Everything else,
  including the oracle replay tests, runs without Docker.

```sh
cargo test --workspace
cargo clippy --workspace --all-targets -- -D warnings
cargo fmt --all
```

## Lints

The workspace denies Clippy's `string_slice` (with `char_indices_as_byte_indices`
and `sliced_string_as_bytes`): text from projects can contain any Unicode,
and a byte index that is not a character boundary panics. Prefer APIs that
cannot split a character (`strip_prefix`, `split_once`, `char_indices`,
`get(..)`, `split_at_checked`, `TextRange::slice` and `TextRange::get`).
Keep a byte slice (or `split_at`) only where the index is a boundary by
construction (it comes from `find` of a character on the same string, from
the lexer, or follows an ASCII byte), with `#[allow(clippy::string_slice)]`
on that statement and a one-line comment saying why; never on a module or
crate. `tests/stress.rs` (`unicode_everywhere_does_not_panic`, and the
ignored `unicode_everywhere_long`) runs every command over generated
workspaces full of such characters.

## Workspace layout

| Path | Contents |
|---|---|
| `crates/iglint_text` | Offsets, ranges, line index, text edits. |
| `crates/iglint_json` | Lossless JSON parser with spans and minimal edits. |
| `crates/iglint_python` | Jython 2.7 lexer, parser and AST. |
| `crates/iglint_python_semantic` | Scopes, bindings, flow analysis for scripts. |
| `crates/iglint_expression` | Ignition expression language, property and tag references. |
| `crates/iglint_stubs` | Generated Ignition 8.3 facts (`system.*`, expression functions, components). |
| `crates/iglint_project` | Project discovery, resource model, Perspective view model, project index. |
| `crates/iglint_linter` | Diagnostics, rule registry, the rules, suppressions, baseline, fixes, driver, cache. |
| `crates/iglint_config` | `iglint.toml` and ign-lint configuration loading, `migrate`. |
| `crates/iglint_server` | Language server. |
| `crates/iglint` | The binary: CLI, output formats, ign-lint compatible mode, rule docs generator. |
| `editors/vscode` | VS Code extension. |
| `oracle/` | Differential tests against the parsers inside the Ignition image. |
| `stubs-gen/` | Regenerates `crates/iglint_stubs` from a running gateway. |
| `docs/rules/` | Rule reference, generated from the rule metadata. |
| `docs/compat/` | ign-lint behaviour specification and known differences. |
| `docs/internal/` | Ignition file format notes and the rule catalogue. |

## Adding a rule

1. **Pick a code and name.** Add a row to the catalogue in
   [docs/internal/rules.md](docs/internal/rules.md) under the right category
   (`VIEW`, `NAME`, `BIND`, `SCR`, `REF`, `PERF`, `SEC`, `A11Y`, `STYLE`,
   `TAG`). Codes and names are stable once released.
2. **Create the module**
   `crates/iglint_linter/src/rules/<category>/<rule_name>.rs` with:
   * `pub static META: RuleMeta` (`crate::registry`): `code`, `name`,
     `category`, `default_severity`, `default_enabled`, `fix`
     (`FixAvailability`), a one-line `summary`, the Markdown `explanation`,
     `options` (`OptionMeta`), and `ign_lint_names` /
     `ign_lint_option_map` if ign-lint has an equivalent. Fill the rest with
     `..RuleMeta::DEFAULT`.
   * `pub static RULE: RuleImpl` (`crate::rule`): `meta: &META`, `phase`
     (`Phase::Local` for checks on one resource, `Phase::Project` when the
     rule needs `cx.index()`), `hooks`, and `options` (`None`, or a parser
     `fn(&OptionReader) -> Result<ParsedOptions, String>`).
   * A unit struct implementing one or more context traits, listed in
     `hooks` as `Hook::<Context>(&MyRule)`: `ResourceRule`, `ViewRule`,
     `ComponentRule`, `BindingRule`, `ExpressionRule`, `ScriptRule`,
     `ProjectRule`. Each gets a `&mut Checker`; report with
     `cx.report(range, message)` (or `report_in`, `report_expression`,
     `report_script`, and the `report` helper in `rules::script` for
     scripts), then chain `.node_path(..)`, `.label(..)` or `.fix(..)`. Read
     parsed options with `cx.options::<MyOptions>()`.
3. **Register it**: `pub mod <rule_name>;` in the category's `mod.rs`, and the
   module path in the `rules![...]` list of
   `crates/iglint_linter/src/rules/mod.rs`, in code order.
4. **Document it.** The explanation must have `## What it does`,
   `## Why is this bad?` and `## Example` sections and mention every option
   in backticks; option defaults must parse. The test
   `rules_are_sorted_unique_and_consistent` checks all of this. Option names
   are kebab-case.
5. **Test it** in a `#[cfg(test)] mod tests` in the same file: view and
   resource rules with `rules::test_support::Fixture` (a project written to a
   temporary directory and linted through the real engine), script rules
   with `rules::script::testing` (`lint_module`, `view(...)`). Cover what is
   reported, what is not (the false positives you thought of), and fixes.
6. **Regenerate the docs**: `cargo run -p iglint -- generate-docs`, and
   commit `docs/rules/`. The test `rule_docs_are_up_to_date` (and CI) fail
   when the pages are stale.

A rule that cannot decide with certainty stays silent, or reports only with
`strict` and says so in its documentation.

## Tests and snapshots

* `cargo test --workspace` runs everything that needs no Docker and no
  private data.
* Snapshot tests use [insta](https://insta.rs). After an intended change,
  review with `cargo insta review` (needs `cargo install cargo-insta`) or
  accept with `INSTA_UPDATE=always cargo test -p <crate>` and read the diff.
* ign-lint parity: `crates/iglint/tests/ign_lint_parity.rs` compares
  `iglint ign-lint` with outputs recorded from ign-lint 0.6.4
  (`crates/iglint_linter/tests/fixtures/ign-lint/`). Known differences are
  in `docs/compat/ign-lint-differences.md`.

## Oracles

The parsers are tested against Ignition's own parsers, run inside the
`inductiveautomation/ignition:8.3.3` Docker image (override with
`IGLINT_ORACLE_IMAGE`). The recorded outputs are committed, so the replay
tests run without Docker:

```sh
cargo test -p iglint_python --test oracle --test float_repr
cargo test -p iglint_python_semantic --test compile_oracle --test scope_oracle --test implicit_reads_oracle --test method_size_oracle
cargo test -p iglint_expression --test oracle
```

To record new outputs (needs Docker):

* `oracle/record-expected.sh` records Jython's AST dumps and syntax errors for
  `crates/iglint_python/tests/oracle/*.cases`.
* `oracle/run-oracle.sh [--tokens | --compile] ROOT LIST OUT` runs Jython on
  any set of files; `oracle/extract-stdlib.sh DEST` and
  `oracle/fuzz.sh SEED COUNT SOURCE_DIR` run larger differential tests.
* `oracle/expression/run.sh record` records the expression, property
  reference and tag path cases; `oracle/expression/run.sh random [SEED]
  [COUNT]` runs a fresh random batch.
* `oracle/scope/run.sh record` records how Jython's compiler reaches every
  name of `oracle/scope/hand.cases` and a stratified sample of generated
  programs (`crates/iglint_python_semantic/tests/oracle/scope.*`);
  `oracle/scope/run.sh random [SEED] [COUNT]` runs a fresh batch against
  the semantic model.
* `oracle/compile_cross.py all OUT` writes the cross product of control
  flow and compile errors; run it with `oracle/run-oracle.sh --compile`
  and the ignored test `compile_errors_match_jython_full_run`
  (`IGLINT_COMPILE_CASES`, `IGLINT_COMPILE_EXPECTED`); `compile_cross.py
  stratify` makes the replayed sample `tests/oracle/compile_cross.*`.
* `oracle/run-oracle.sh --reads` records the names running each source
  looks up in its module's globals
  (`tests/oracle/implicit_reads.expected`).
* `oracle/run-oracle.sh --method-size` records the bytecode length of
  every code unit as Jython's code generator emits it, and whether
  `compile()` succeeds; `tests/method_size_oracle.rs` (in
  `iglint_python_semantic`) writes its sources (`write_sources`) and
  checks `SCR002`'s lower bound against
  `tests/oracle/method_size.expected`.
* `oracle/run-oracle.sh --run` records what running each source as a
  library module does (output, exception with its line, globals); the
  differential test of the layout-only safe fixes,
  `rules::script::layout_oracle` in `iglint_linter`, writes its sources,
  compares the records and replays a committed sample
  (`tests/fixtures/layout-fix/`).
* `oracle/run-oracle.sh --library-routes` records the members of
  Ignition's library module and package objects and of Jython's
  function, method, class, instance and generator objects, and what each
  route of `crates/iglint_linter/tests/fixtures/library-routes/*.cases`
  does (scripts in a library module, `runScript` expressions through
  Ignition's own function factory, and in `chains.cases` paths through
  the imports of libraries each case defines); `rules::refs::library_routes`
  replays it. `stubs-gen/code_functions.py` lists the expression
  function classes of the image that can run Python.
* `oracle/format-spec.sh all OUT` records Jython's verdict on every
  generated `str.format` spec and literal argument (SCR112); the replayed
  sample in `crates/iglint_linter/tests/fixtures/format-spec/` is
  `python3 oracle/format_spec_cases.py stratify OUT 10 4000`.
* `oracle/format-unicode.sh all OUT` records Jython's verdict on
  formatting that mixes byte strings and unicode (SCR112); the replayed
  sample `crates/iglint_linter/tests/fixtures/format-spec/unicode.tsv` is
  `python3 oracle/format_unicode_cases.py pick OUT 14 5000`.
* `stubs-gen/run.sh` starts a throwaway gateway, regenerates
  `crates/iglint_stubs` and rewrites `stubs-gen/CROSSCHECK.md`. Review the
  diff of the data files.

## Private corpora

Real Ignition projects used during development belong to customers. Nothing
derived from them may enter the repository: no snippets, file, view or
component names, identifiers, snapshots or counts beyond aggregates.

* Pass a private project only through `IGLINT_PRIVATE_CORPUS`; never write
  its path in code, tests or docs.
* Tests that use it are `#[ignore]` and print aggregates only, e.g.
  `IGLINT_PRIVATE_CORPUS=<path> cargo test --release -p iglint --test private_corpus -- --ignored --nocapture`
  and `oracle/private-corpus.sh`.
* Public test material is hand-written, generated, or taken from the
  standard library that ships with Jython.
* `scripts/check-no-leak.sh` fails when tracked files contain a marker from
  `$IGLINT_LEAK_MARKERS` (default `~/.config/iglint/leak-markers.txt`, one
  case-insensitive string per line, kept outside the repository). Run it
  before every push; CI runs it with markers from a repository secret.

## Benchmarks

Performance is a feature: a change that makes iglint slower needs a reason.
Compare the release build before and after the change with the benchmark
harness in `bench/` (see [bench/README.md](bench/README.md)), and include the
numbers in the pull request when the change touches the parsers, the project
model or the driver.

## Commits and pull requests

* One topic per commit. The subject is a short imperative or descriptive
  summary; start with the rule codes when the change is about rules
  (`SCR305 unresolved component references`).
* Keep `cargo fmt`, `cargo clippy -- -D warnings`, tests and the generated
  docs clean in every commit.
* Add user-visible changes to the "Unreleased" section of
  [CHANGELOG.md](CHANGELOG.md).
* By contributing you agree that your contribution is dual licensed under
  the MIT and Apache-2.0 licenses, like the rest of iglint.

## Releasing

1. Update the version in `Cargo.toml` (`[workspace.package]`) and
   `editors/vscode/package.json`, and move the "Unreleased" changelog entries
   under the new version.
2. Tag the commit `vX.Y.Z` and push the tag. The release workflow builds the
   binaries, wheels, source distribution and VS Code packages, attaches them
   to a draft GitHub release and publishes the wheels to PyPI.
