---
title: CI integration
description: Run iglint in GitHub Actions, GitLab CI, Azure Pipelines and other CI systems, with annotations, SARIF code scanning and Code Quality reports.
---

iglint is a single binary with a clear exit status, so any CI system can run it. What differs is how findings are shown: as annotations on the pull request, in a code scanning dashboard, or in a test report. Pick the matching `--output-format` (all formats are described in [Output formats](/reference/output-formats/)).

## The gate

```sh
iglint check
```

- Exit status `0`: no finding at or above the `--error-on` level (default `warning`).
- Exit status `1`: at least one such finding.
- Exit status `2`: usage, configuration or internal error.

Useful variations:

| Option | Effect |
|---|---|
| `--error-on error` (or `--ignore-warnings`) | only errors fail the build |
| `--error-on info` | every finding fails the build |
| `--exit-zero` | never fail on findings (for a report-only job) |
| `-o FILE` / `--output-file FILE` | write the report to a file instead of standard output |
| `-q` / `--quiet` | findings only, no summary |
| `--no-cache` | do not read or write `.iglint_cache` (see [caching in CI](#caching-in-ci)) |

On an existing project, commit a [baseline](/guides/baseline/) first so that only new findings fail.

## Installing iglint in the job

The examples below download the Linux x86_64 binary of a pinned release from [GitHub releases](https://github.com/Mustry-Solutions/iglint/releases) (it is statically linked and runs on any Linux image) and put it on the `PATH`. The archive `iglint-v0.1.0-x86_64-unknown-linux-musl.tar.gz` holds a directory of the same name with the `iglint` binary. Change the version in one place to upgrade. Each archive has a `.sha256` file next to it, which the GitHub Actions example checks; for a stronger check, compare against a hash you recorded yourself.

Once iglint is on PyPI (planned, not published yet), `pip install iglint==0.1.0` will do the same in an image with Python.

## GitHub Actions

Annotations on the changed lines of a pull request, plus code scanning alerts:

```yaml
# .github/workflows/iglint.yml
name: iglint
on: [pull_request, push]

jobs:
  iglint:
    runs-on: ubuntu-latest
    permissions:
      contents: read
      security-events: write   # only for the SARIF upload
    steps:
      - uses: actions/checkout@v4
      - name: Install iglint
        env:
          IGLINT_VERSION: v0.1.0
        run: |
          cd "$RUNNER_TEMP"
          archive="iglint-${IGLINT_VERSION}-x86_64-unknown-linux-musl"
          url="https://github.com/Mustry-Solutions/iglint/releases/download/${IGLINT_VERSION}/${archive}.tar.gz"
          curl -sSfLO "$url"
          curl -sSfLO "$url.sha256"
          sha256sum -c "${archive}.tar.gz.sha256"
          tar -xzf "${archive}.tar.gz"
          echo "$RUNNER_TEMP/${archive}" >> "$GITHUB_PATH"
      # Annotations, and the gate.
      - run: iglint check --output-format github
      # Code scanning (optional): a SARIF report, uploaded even when the gate failed.
      - run: iglint check --output-format sarif --output-file iglint.sarif --exit-zero
        if: always()
      - uses: github/codeql-action/upload-sarif@v3
        if: always()
        with:
          sarif_file: iglint.sarif
```

The `github` format prints workflow commands, which GitHub turns into annotations:

```text
::error file=projects/demo-hmi/com.inductiveautomation.perspective/views/Line/Overview/view.json,line=84,col=75,endLine=84,endColumn=94,title=REF001::View `Line/PumpDetial` does not exist
::warning file=projects/demo-hmi/com.inductiveautomation.perspective/views/Line/Overview/view.json,line=56,col=43,endLine=56,endColumn=46,title=BIND004::`now` polls every 1000 ms, faster than the minimum of 10000 ms
```

Errors become `::error`, warnings `::warning` and info findings `::notice`.

## GitLab CI

GitLab shows Code Quality reports in merge requests:

```yaml
# .gitlab-ci.yml
iglint:
  image: python:3.12   # any Linux image with curl
  variables:
    IGLINT_VERSION: v0.1.0
    IGLINT_ARCHIVE: iglint-${IGLINT_VERSION}-x86_64-unknown-linux-musl
  script:
    - curl -sSfL -o /tmp/iglint.tar.gz "https://github.com/Mustry-Solutions/iglint/releases/download/${IGLINT_VERSION}/${IGLINT_ARCHIVE}.tar.gz"
    - tar -xzf /tmp/iglint.tar.gz -C /usr/local/bin --strip-components=1 "${IGLINT_ARCHIVE}/iglint"
    - iglint check --output-format gitlab --output-file gl-code-quality-report.json
  artifacts:
    when: always
    reports:
      codequality: gl-code-quality-report.json
```

## Azure DevOps Pipelines

The `azure` format prints `##vso[task.logissue]` logging commands, which show up as errors and warnings in the build summary:

```yaml
# azure-pipelines.yml
pool:
  vmImage: ubuntu-latest
variables:
  IGLINT_VERSION: v0.1.0
steps:
  - script: |
      archive="iglint-$(IGLINT_VERSION)-x86_64-unknown-linux-musl"
      curl -sSfL "https://github.com/Mustry-Solutions/iglint/releases/download/$(IGLINT_VERSION)/${archive}.tar.gz" \
        | tar -xz -C "$(Agent.TempDirectory)"
      echo "##vso[task.prependpath]$(Agent.TempDirectory)/${archive}"
    displayName: Install iglint
  - script: iglint check --output-format azure
    displayName: iglint
```

```text
##vso[task.logissue type=error;sourcepath=projects/demo-hmi/com.inductiveautomation.perspective/views/Line/Overview/view.json;linenumber=84;columnnumber=75;code=REF001]View `Line/PumpDetial` does not exist
```

To also publish the findings as test results, add a JUnit report:

```yaml
  - script: iglint check --output-format junit --output-file iglint-junit.xml --exit-zero
    displayName: iglint (JUnit report)
    condition: always()
  - task: PublishTestResults@2
    condition: always()
    inputs:
      testResultsFormat: JUnit
      testResultsFiles: iglint-junit.xml
```

## Other CI systems

- **JUnit XML** (`--output-format junit`) works with Jenkins, TeamCity, Bitbucket Pipelines, CircleCI and most others: one test case per finding, grouped by file.
- **SARIF** (`--output-format sarif`) is read by many code scanning and review tools.
- **JSON** and **JSON lines** (`json`, `json-lines`) are for your own scripts and dashboards.

## Caching in CI

iglint caches results in `.iglint_cache` in the project root. In CI a cold run is already fast (about 0.16 s on a 500-view synthetic corpus, 0.6 s on 2 CPUs), so caching across jobs is rarely worth it. Either keep the directory between runs with your CI's cache feature, or pass `--no-cache` to avoid writing it.

## Running only on changed files

iglint can lint a list of files (`iglint check path/a.py path/b/view.json`), and still analyses the whole workspace around them, so cross-resource rules see the full project. For pull request gates, linting everything is usually simpler and still fast; per-file runs are what the [pre-commit hook](/guides/pre-commit/) does.
