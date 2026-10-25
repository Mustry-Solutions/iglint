---
title: Contributing
description: How to contribute to iglint, and how this documentation site is built.
---

Contributions are welcome: bug reports with a small invented example, new rules, fixes and documentation. The full guide is [`CONTRIBUTING.md`](https://github.com/Mustry-Solutions/iglint/blob/main/CONTRIBUTING.md) in the repository; read [`DESIGN.md`](https://github.com/Mustry-Solutions/iglint/blob/main/DESIGN.md) first. Its principles decide most reviews: no false positives, exact Jython, byte-exact fixes, precise locations, and no customer data.

In short:

```sh
cargo test --workspace
cargo clippy --workspace --all-targets -- -D warnings
cargo fmt --all
```

- **Adding a rule**: pick a code and name, write the rule with its `RuleMeta` (summary, explanation with "What it does", "Why is this bad?" and "Example", options), register it, test what it reports and what it must not report, and regenerate the documentation with `cargo run -p iglint -- generate-docs`.
- **Never commit customer data**: no snippets, names or identifiers from real projects. Examples and test fixtures are invented.
- **Performance is a feature**: changes to the parsers, the project model or the driver come with benchmark numbers.

## This documentation site

The site is built with [Astro Starlight](https://starlight.astro.build) from the `site/` directory of the repository (Node.js 22.23 or newer):

```sh
cd site
npm install
npm run dev        # http://localhost:4321, reloads on change
npm run build      # static site in site/dist
```

- **Hand-written pages** are Markdown files in `site/src/content/docs/`.
- **Rule pages are generated.** `iglint generate-docs` writes `docs/rules/*.md` from the rule metadata in the code, and CI fails when they are out of date. Before every `dev` and `build`, `site/scripts/sync-docs.mjs` copies them into the site (with the rules overview and the sidebar built from `docs/rules/README.md`), together with `docs/compat/ign-lint-differences.md`, `docs/internal/fix-safety.md` and `CHANGELOG.md`. The copies are not committed: to change a rule page, change the rule's metadata and run `generate-docs`.
- **The tutorial** uses the invented project in `site/examples/quickstart`. After a change to the rules, `site/scripts/run-example.sh [path/to/iglint]` replays it and prints every command with its output, to compare with `getting-started.md`.
- **The site's address** is set in one place, `site/site.config.mjs`.
