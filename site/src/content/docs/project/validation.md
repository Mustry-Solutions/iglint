---
title: How iglint is validated
description: Oracle testing against Ignition's own Jython and expression parser, generated facts from a real gateway, fuzzing, the ign-lint parity suite and independent review.
---

"A false positive is a bug" is only a promise if it is tested. This page summarises how; the details are in the repository (`oracle/`, `stubs-gen/`, `bench/`, `CONTRIBUTING.md`).

## Oracles: Ignition's own parsers as the reference

The parsers and the compiler checks are tested **differentially** against the real thing: the Jython and the expression parser inside the `inductiveautomation/ignition:8.3.3` Docker image. The scripts in `oracle/` run them on a set of inputs and record their output; the recorded outputs are committed, so the replay tests run in CI without Docker.

| What | Compared with Ignition's | Size |
|---|---|---|
| Jython parser | syntax trees (`ast.dump` with positions) and syntax errors, for hand-written cases, the Python standard library shipped with Jython, and fuzzed programs | case files plus larger runs on demand |
| Compiler errors (`SCR002`) | whether Jython's compiler accepts the code, with its message | a cross product of control-flow and error constructs: 13,202 cases with no difference |
| Compiler warnings (`SCR003`) | the `SyntaxWarning`s Jython issues | recorded set |
| Name resolution | which frame accessor Jython's code generator picks for every name, and how it mangles private names | hand-written cases and generated programs |
| Script size limits (`SCR002`) | the bytecode length of every code unit and whether it compiles | about 2,300 generated programs for the lower bounds, 5,794 for the upper bounds |
| String formatting (`SCR112`) | whether a `str.format` spec or `%` formatting raises for a literal argument | 5,414,400 generated cases, a stratified sample of 4,000 replayed in the tests |
| Expression parser | parse results of expressions, property references and tag paths | recorded cases and random batches |

Some behaviour that iglint relies on was verified on a running 8.3.3 gateway rather than a parser: which names Ignition injects into each kind of script, the header it wraps around Perspective scripts, and that a gateway event file runs the last binding of its handler name (`SCR307`).

## Facts generated from a real gateway

The `system.*` scripting API (packages, functions, overloads, parameters and scopes), the expression functions and the Perspective component schemas are not hand-written. `stubs-gen/run.sh` starts a throwaway 8.3.3 gateway, introspects it, and regenerates the data in `crates/iglint_stubs`. The result is cross-checked against the community type stubs of [ignition-api](https://github.com/ignition-devs/ignition-api) (`stubs-gen/CROSSCHECK.md`), which surfaced 155 functions visible in the Designer that the community stubs do not list.

## Fuzzing and stress tests

- **Randomised tests** for the JSON and expression parsers: no panics on arbitrary input, exact round trips of generated documents, and edits that always produce the expected tree.
- **Differential fuzzing** of the Jython parser (`oracle/fuzz.sh`): mutated snippets of the standard library and random programs, parsed by both iglint and Jython.
- **Stress tests** run in CI on Linux and Windows: generated inputs at and beyond the nesting limits, through the release binary, the ign-lint mode and the language server, and workspaces full of unusual Unicode, to make sure no input makes iglint crash.
- **Cache correctness**: every one-file change of the fixture workspace is checked against an uncached run.

## The ign-lint parity suite

`crates/iglint/tests/ign_lint_parity.rs` compares `iglint ign-lint` with output recorded from real ign-lint 0.6.4, for every fixture view and every configuration variant. Every accepted difference is an entry with its reason in a differences file, and an entry that no longer matches fails the test. The differences are documented in [Differences from ign-lint](/reference/ign-lint-differences/).

On larger inputs, `bench/parity.py` runs both linters with the same flags and classifies every finding as identical, expected (a documented difference) or unexplained:

| Corpus | Identical | Expected | Unexplained |
|---|---|---|---|
| synthetic, 50 views | 24 | 7 | 0 |
| synthetic, 500 views | 218 | 108 | 0 |
| synthetic, 5,000 views | 2,247 | 1,146 | 0 |
| a production customer project (aggregates only) | 1,463 | 915 | 0 |

pylint findings are compared separately, by message code, because they depend on the installed pylint and on how scripts are combined.

## Tests and CI

The Rust workspace has more than 1,100 test functions, plus snapshot tests of the command-line output. Every pull request runs formatting, Clippy with warnings as errors, the tests on Linux, macOS and Windows, the oracle replay, the stress tests, a build with the minimum supported Rust version, a check that the generated rule documentation is up to date, the VS Code extension's tests, a wheel build, and a scan for data from private projects.

## Independent review

Before the first release, the code went through 24 rounds of independent review, ending with an approval in round 24. Reviewers tried to break the rules and, above all, the safe fixes; each finding was fixed with a regression test named after the condition it covers. The [fix safety design notes](/internals/fix-safety/) record what the rounds found and why the safe-fix design ended up default-deny.

## Private data

Real projects used during development belong to customers. Nothing derived from them is committed: no snippets, names or identifiers, only aggregate numbers such as file counts and timings. Tests that need such a project are opt-in and print aggregates only, and CI scans every push and pull request for markers of private data. All examples in this documentation are invented.
