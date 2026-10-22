#!/usr/bin/env python3
"""Compare ign-lint 0.6.4 with `iglint ign-lint` on one corpus.

Runs both with the same ign-lint flags (from the corpus directory, each with
its own --results-output), parses the two results reports, matches findings
per file, rule and node path, and classifies every difference:

  expected       documented in docs/compat/ign-lint-differences.md
                 (the category names the section)
  ign-lint-only  unexplained: a potential iglint false negative
  iglint-only    unexplained: a potential iglint false positive

Usage:
  bench/parity.py CORPUS [--out DIR] [--private] [--list] -- IGN-LINT FLAGS...

Example:
  bench/parity.py /tmp/iglint-corpora/medium -- \\
      --config bench/configs/rules-only.json --files 'projects/**/view.json'

With --private (or when CORPUS is $IGLINT_PRIVATE_CORPUS) only aggregate
counts are printed; the detailed lists go to --out, which must be outside
the iglint repository (default /tmp/iglint-parity-private).
"""

import argparse
import collections
import json
import os
import re
import subprocess
import sys
from pathlib import Path

REPO = Path(__file__).resolve().parent.parent
DEFAULT_IGN_LINT = os.environ.get("IGN_LINT", "/tmp/ignlint-venv/bin/ign-lint")
DEFAULT_IGLINT = os.environ.get("IGLINT", str(REPO / "target/release/iglint"))

ABBREVIATIONS = (
    "API HTTP HTTPS XML JSON SQL URL URI UUID CPU GPU RAM SSD HDD PDF CSV ZIP GIF PNG JPG JPEG "
    "SVG CSS HTML JS TS PHP ASP JSP CGI FTP SSH TCP UDP IP DNS DHCP VPN SSL TLS JWT CRUD REST "
    "SOAP AJAX DOM UI UX GUI CLI OS iOS macOS AWS GCP IBM AI ML NLP OCR QR RFID NFC GPS LED "
    "LCD OLED CRT ID"
).split()

FILE_LINE = re.compile(r"^(?:❌|⚠️|✅)\s+(.+)$")
SECTION = re.compile(r"^(ERRORS|WARNINGS) \(\d+ total\):$")
RULE = re.compile(r"^  (\w+):$")
PYLINT_CAT = re.compile(r"^    Pylint - \w+ \((\w)\):$")
MESSAGE = re.compile(r"^    • (.*)$")
PYLINT_MESSAGE = re.compile(r"^      • (.*)$")


def parse_results(path):
    """Findings as a list of dicts: file, severity, rule, path, text, raw."""
    findings = []
    current = None
    severity = None
    rule = None
    lines = Path(path).read_text(encoding="utf-8").splitlines()
    for i, line in enumerate(lines):
        if line.startswith("📦") or line.strip() == "END OF RESULTS":
            break
        prev = lines[i - 1] if i else ""
        m = FILE_LINE.match(line)
        if m and prev.startswith("-" * 20):
            current, severity, rule = m.group(1).strip(), None, None
            continue
        m = SECTION.match(line)
        if m:
            severity = "error" if m.group(1) == "ERRORS" else "warning"
            continue
        m = RULE.match(line)
        if m:
            rule = m.group(1)
            continue
        if PYLINT_CAT.match(line):
            continue
        m = MESSAGE.match(line) or PYLINT_MESSAGE.match(line)
        if m and current and rule:
            raw = m.group(1)
            node, sep, text = raw.partition(": ")
            if not sep:
                node, text = "", raw
            findings.append(
                {"file": current, "severity": severity, "rule": rule, "path": node, "text": text, "raw": raw}
            )
    return findings


def normalise(f):
    """Message text with documented wording differences removed."""
    text = f["text"]
    if f["rule"] == "BadComponentReferenceRule":
        # Bug 18: ign-lint counts `../` also as `./`; only the content type
        # and the fact of a traversal are compared.
        text = re.sub(r" contains '.*?'( and \d+ other object traversal pattern\(s\))? which", " contains <pattern> which", text)
    if f["rule"] == "NamePatternRule":
        # Bug 8: suggestions differ; the finding is the same.
        text = re.sub(r"\. Suggestion: .*$", "", text)
        text = re.sub(r" \(.*suggest.*\)$", "", text)
    return " ".join(text.split())


class ViewCache:
    def __init__(self, root):
        self.root = Path(root)
        self.cache = {}

    def get(self, file):
        if file not in self.cache:
            try:
                self.cache[file] = json.loads((self.root / file).read_text(encoding="utf-8"))
            except (OSError, ValueError):
                self.cache[file] = None
        return self.cache[file]


def prop_config(view, path):
    """propConfig entry for an ign-lint node path such as `custom.x` or
    `params.y` (view level only)."""
    if not isinstance(view, dict):
        return None
    m = re.match(r"^(custom|params)\.([^.\[]+)", path)
    if not m:
        return None
    return (view.get("propConfig") or {}).get(f"{m.group(1)}.{m.group(2)}")


def has_abbreviation(name):
    up = name.upper()
    return any(a.upper() in up for a in ABBREVIATIONS)


def classify(f, side, views):
    """Category for an unmatched finding, or None when unexplained."""
    rule, path, text = f["rule"], f["path"], f["text"]
    if rule == "PylintScriptRule":
        return "not compared: PylintScriptRule (SCR rules)"
    if rule == "ComponentReferenceValidationRule":
        if side == "ign-lint" and (".scripts." in path or ".events." in path or "onChange" in path or "self." in text):
            return "not compared: ComponentReferenceValidationRule script part (SCR305)"
        if side == "iglint" and re.search(r"\b(custom|params)\b", text) and "property" in text.lower():
            return "BIND005: custom/params existence"
        if not path.startswith("root"):
            return "BIND005: view-level bindings resolve from the view (bug 20)"
    if rule == "BadComponentReferenceRule":
        if side == "iglint" and "extensionFunctions" in path:
            return "whole run: extension functions are scripts (bug 1)"
    if rule == "NamePatternRule":
        m = re.search(r"Name '([^']*)'", text)
        if m and has_abbreviation(m.group(1)):
            return "NAME001: abbreviations are whole words (bug 6)"
        if "already in use" in text:
            return "NAME001: no self-collision message (bug 7)"
    if rule == "PollingIntervalRule" and side == "ign-lint":
        if re.search(r"(?<![A-Za-z_])now\s*\(\s*\)", text) or re.search(r"[A-Za-z_]now\s*\(", text):
            return "BIND004: expressions are parsed (bug 10)"
        if re.search(r"now\s*\(\s*(-|0\s*\))", text) or re.search(r"now\s*\(\s*[^\d\s)]", text):
            return "BIND004: dynamic or non-positive rates"
        if ".references." in path:
            return "BIND004: indirect references are not expressions"
    if rule == "UnusedCustomPropertiesRule":
        view = views.get(f["file"])
        cfg = prop_config(view, path) or {}
        if side == "ign-lint" and isinstance(cfg, dict):
            if cfg.get("onChange"):
                return "VIEW015: property with a change script is used"
            if cfg.get("paramDirection") == "output":
                return "VIEW015: output parameters are VIEW019's concern"
        if side == "iglint":
            return "VIEW015: exact reference matching (bugs 21-23)" if view is not None and _substring_use(view, path) else None
    if rule == "ExcessiveContextDataRule" and side == "iglint":
        return "VIEW006: complete measurement (bug 24)"
    return None


def _substring_use(view, path):
    """True when the property name is a substring of a longer referenced
    name, which ign-lint counts as a use (bug 21)."""
    m = re.match(r"^(?:.*\.)?(custom|params)\.([^.\[]+)", path)
    if not m:
        return False
    name = m.group(2)
    text = json.dumps(view)
    return re.search(rf"{m.group(1)}\.{re.escape(name)}\w", text) is not None


def run(cmd, cwd, log):
    with open(log, "w", encoding="utf-8") as out:
        return subprocess.run(cmd, cwd=cwd, stdout=out, stderr=subprocess.STDOUT).returncode


def main():
    ap = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    ap.add_argument("corpus")
    ap.add_argument("--out", help="directory for reports and difference lists")
    ap.add_argument("--private", action="store_true", help="print aggregates only")
    ap.add_argument("--list", action="store_true", help="print unexplained differences (not with --private)")
    ap.add_argument("--ign-lint", default=DEFAULT_IGN_LINT)
    ap.add_argument("--iglint", default=DEFAULT_IGLINT)
    ap.add_argument("--reuse", action="store_true", help="reuse existing reports in --out")
    argv = sys.argv[1:]
    split = argv.index("--") if "--" in argv else len(argv)
    a = ap.parse_args(argv[:split])
    flags = argv[split + 1 :]
    corpus = Path(a.corpus).resolve()
    private_env = os.environ.get("IGLINT_PRIVATE_CORPUS")
    private = a.private or (private_env and Path(private_env).resolve() == corpus)
    out = Path(a.out or ("/tmp/iglint-parity-private" if private else REPO / "bench/results/parity" / corpus.name)).resolve()
    if private and (out == REPO or REPO in out.parents):
        sys.exit("parity: --out must be outside the iglint repository for a private corpus")
    out.mkdir(parents=True, exist_ok=True)
    # Flags that name files are resolved against the caller's directory.
    resolved = []
    for i, f in enumerate(flags):
        prev = flags[i - 1] if i else ""
        if prev in ("--config", "--whitelist") and Path(f).exists():
            f = str(Path(f).resolve())
        resolved.append(f)
    ign_report, ig_report = out / "ign-lint.results.txt", out / "iglint.results.txt"
    if not a.reuse:
        for report in (ign_report, ig_report):
            if report.exists():
                report.unlink()
        rc1 = run([a.ign_lint, *resolved, "--results-output", str(ign_report)], corpus, out / "ign-lint.log")
        rc2 = run([a.iglint, "ign-lint", *resolved, "--results-output", str(ig_report)], corpus, out / "iglint.log")
        print(f"exit codes: ign-lint {rc1}, iglint {rc2}")

    left, right = parse_results(ign_report), parse_results(ig_report)
    views = ViewCache(corpus)

    def key(f, norm):
        return (f["file"], f["rule"], f["path"], normalise(f) if norm else f["text"])

    stats = collections.defaultdict(collections.Counter)
    diffs = []
    severity_mismatch = 0
    remaining = {"ign-lint": [], "iglint": []}
    # Pass 1: exact, pass 2: normalised message.
    pool = collections.defaultdict(list)
    for f in right:
        pool[key(f, False)].append(f)
    unmatched_left = []
    for f in left:
        k = key(f, False)
        if pool.get(k):
            g = pool[k].pop()
            stats[f["rule"]]["identical"] += 1
            severity_mismatch += f["severity"] != g["severity"]
        else:
            unmatched_left.append(f)
    rest_right = [f for fs in pool.values() for f in fs]
    pool2 = collections.defaultdict(list)
    for f in rest_right:
        pool2[key(f, True)].append(f)
    for f in unmatched_left:
        k = key(f, True)
        if pool2.get(k):
            pool2[k].pop()
            stats[f["rule"]]["expected"] += 1
            stats[f["rule"]]["expected: wording"] += 1
        else:
            remaining["ign-lint"].append(f)
    remaining["iglint"] = [f for fs in pool2.values() for f in fs]

    categories = collections.Counter()
    for side, fs in remaining.items():
        for f in fs:
            cat = classify(f, side, views)
            if cat:
                stats[f["rule"]]["expected"] += 1
                categories[cat] += 1
            else:
                stats[f["rule"]][f"{side}-only"] += 1
                diffs.append({"side": side, **f})

    rules = sorted(stats)
    print(f"\nfindings: ign-lint {len(left)}, iglint {len(right)}; severity mismatches among identical: {severity_mismatch}")
    print(f"{'rule':38} {'identical':>9} {'expected':>9} {'ign-lint-only':>13} {'iglint-only':>11}")
    for r in rules:
        s = stats[r]
        print(f"{r:38} {s['identical']:>9} {s['expected']:>9} {s['ign-lint-only']:>13} {s['iglint-only']:>11}")
    if stats and any(stats[r]["expected: wording"] for r in rules):
        categories["message wording (bugs 8, 18)"] += sum(stats[r]["expected: wording"] for r in rules)
    print("\nexpected differences by category:")
    for c, n in categories.most_common():
        print(f"  {n:6}  {c}")
    unexplained = collections.Counter((d["side"], d["rule"], _shape(d)) for d in diffs)
    print("\nunexplained differences by side, rule and message shape:")
    for (side, rule, shape), n in unexplained.most_common():
        print(f"  {n:6}  {side:13} {rule:34} {shape if not private else ''}")
    (out / "differences.json").write_text(json.dumps(diffs, indent=1, ensure_ascii=False), encoding="utf-8")
    print(f"\nunexplained: {len(diffs)} (details: {out / 'differences.json'})")
    if a.list and not private:
        for d in diffs:
            print(f"{d['side']:13} {d['file']}  {d['rule']}: {d['raw']}")
    return 0


def _shape(d):
    """Message with names and numbers removed, for grouping."""
    t = re.sub(r"'[^']*'", "'…'", d["text"])
    t = re.sub(r"\{[^}]*\}", "{…}", t)
    t = re.sub(r"\d+", "N", t)
    return t[:90]


if __name__ == "__main__":
    sys.exit(main())
