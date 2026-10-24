#!/usr/bin/env python3
"""Compare ign-lint 0.6.4 with `iglint ign-lint` on one corpus.

Runs both with the same ign-lint flags (from the corpus directory, each with
its own --results-output), parses the two results reports, matches findings
per file, rule and node path, and classifies every difference:

  expected       documented in docs/compat/ign-lint-differences.md
                 (the category names the section)
  ign-lint-only  unexplained: a potential iglint false negative
  iglint-only    unexplained: a potential iglint false positive

Matched findings (identical, or equal after normalising documented wording)
are also compared by severity. A severity difference is expected when a
documented severity mapping explains it, else unexplained.

Usage:
  bench/parity.py CORPUS [--out DIR] [--private] [--list] -- IGN-LINT FLAGS...

Example:
  bench/parity.py /tmp/iglint-corpora/medium -- \\
      --config bench/configs/rules-only.json --files 'projects/**/view.json'

With --private (or when CORPUS is $IGLINT_PRIVATE_CORPUS) only aggregate
counts are printed; the detailed lists go to --out, which must be outside
the iglint repository (default ~/.cache/iglint-bench/parity-private).
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
BENCH_HOME = Path(os.environ.get("BENCH_HOME", Path.home() / ".cache/iglint-bench"))
DEFAULT_IGN_LINT = os.environ.get("IGN_LINT", str(BENCH_HOME / "venv/bin/ign-lint"))
DEFAULT_IGLINT = os.environ.get(
    "IGLINT", str(Path(os.environ.get("CARGO_TARGET_DIR", BENCH_HOME / "target")) / "release/iglint")
)

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
    """Findings as a list of dicts: file, severity, rule, category (the
    pylint category letter of a grouped pylint message, else None), path,
    text, raw."""
    findings = []
    current = None
    severity = None
    rule = None
    category = None
    lines = Path(path).read_text(encoding="utf-8").splitlines()
    for i, line in enumerate(lines):
        if line.startswith("📦") or line.strip() == "END OF RESULTS":
            break
        prev = lines[i - 1] if i else ""
        m = FILE_LINE.match(line)
        if m and prev.startswith("-" * 20):
            current, severity, rule, category = m.group(1).strip(), None, None, None
            continue
        m = SECTION.match(line)
        if m:
            severity = "error" if m.group(1) == "ERRORS" else "warning"
            continue
        m = RULE.match(line)
        if m:
            rule, category = m.group(1), None
            continue
        m = PYLINT_CAT.match(line)
        if m:
            category = m.group(1)
            continue
        m = MESSAGE.match(line) or PYLINT_MESSAGE.match(line)
        if not m and findings and line.strip() and not line.startswith(("=", "-", " ")) and findings[-1]["file"] == current:
            # A name with a line break continues the previous message.
            last = findings[-1]
            last["raw"] += "\n" + line
            node, sep, text = last["raw"].partition(": ")
            last["path"], last["text"] = (node, text) if sep else ("", last["raw"])
            continue
        if m and current and rule:
            raw = m.group(1)
            node, sep, text = raw.partition(": ")
            if not sep:
                node, text = "", raw
            findings.append(
                {
                    "file": current,
                    "severity": severity,
                    "rule": rule,
                    "category": category if PYLINT_MESSAGE.match(line) else None,
                    "path": node,
                    "text": text,
                    "raw": raw,
                }
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


# Script uses of a property container that VIEW015 cannot follow
# (`params.get(...)`, subscripts, `self` or the view passed on, aliases).
DYNAMIC_ACCESS = re.compile(
    r"self\.view\.(params|custom)\.get\(|self\.view\.(params|custom)\[|\(self[,)]|, ?self[,)]"
    r"|self\.view\)|= ?self\.view\b|getattr\("
)

# A call of a dotted name that is not rooted at a script's own objects:
# a project library function. When any library module evaluates computed
# strings or reaches a frame, iglint treats such a call as reading every
# name it could reach (rounds 4 and 5), so it does not report the view's
# containers as unused. The root excludes `self`, the Ignition globals and
# the objects a script reaches from them.
LIBRARY_CALL = re.compile(
    r"(?<![\w.$])(?!(?:self|system|event|sys|fpmi|this|value|previousValue|currentValue|payload)\b)"
    r"[A-Za-z_]\w*(?:\.\w+)+\("
)


def resolve(view, path):
    """The JSON value at an ign-lint node path (`custom.a.b`,
    `root.root.children.Name.custom.x`), or None."""
    if not isinstance(view, dict) or not path:
        return None
    parts = re.split(r"\.|\[(\d+)\]", path)
    parts = [p for p in parts if p]
    node = view
    if parts[:2] == ["root", "root"]:
        node, parts = view.get("root"), parts[2:]
    i = 0
    while i < len(parts) and node is not None:
        key = parts[i]
        if key == "children" and isinstance(node, dict) and i + 1 < len(parts):
            name = parts[i + 1]
            kids = node.get("children") or []
            node = next((k for k in kids if (k.get("meta") or {}).get("name") == name), None)
            if node is None and name.isdigit() and int(name) < len(kids):
                node = kids[int(name)]
                # ign-lint writes `children[2].Name`: the name follows the index.
                if i + 2 < len(parts) and isinstance(node, dict) and (node.get("meta") or {}).get("name") == parts[i + 2]:
                    i += 1
            i += 2
            continue
        if isinstance(node, dict):
            node = node.get(key)
        elif isinstance(node, list) and key.isdigit() and int(key) < len(node):
            node = node[int(key)]
        else:
            return None
        i += 1
    return node


def expression_binding(view, path):
    """The expression of the expression binding at an ign-lint `propConfig`
    path (`propConfig.{key}` for the view, `{component}.propConfig.{key}`
    for a component), or None."""
    if not isinstance(view, dict):
        return None
    if path.startswith("propConfig."):
        owner, key = view, path[len("propConfig.") :]
    else:
        head, sep, key = path.partition(".propConfig.")
        if not sep:
            return None
        owner = resolve(view, head)
    if not isinstance(owner, dict):
        return None
    binding = ((owner.get("propConfig") or {}).get(key) or {}).get("binding") or {}
    if binding.get("type") != "expr":
        return None
    expression = (binding.get("config") or {}).get("expression")
    return expression if isinstance(expression, str) else None


# Python attribute access on `self`, which the Ignition expression language
# does not have (it reads properties through `{...}` references).
PYTHON_SELF = re.compile(r"(?<![\w.])self\s*\.")


def python_in_expression(expression, pattern):
    """True when `expression` is Python code (`self.` outside string
    literals and `{...}` references) and the reported traversal `pattern`
    occurs in that code."""
    code = re.sub(r"'(?:[^'\\]|\\.)*'|\"(?:[^\"\\]|\\.)*\"|\{[^}]*\}", "", expression)
    return bool(PYTHON_SELF.search(code)) and pattern in code


def has_abbreviation(name):
    up = name.upper()
    return any(a.upper() in up for a in ABBREVIATIONS)


def classify(f, side, views):
    """Category for an unmatched finding, or None when unexplained."""
    rule, path, text = f["rule"], f["path"], f["text"]
    if rule == "PylintScriptRule":
        return "not compared: PylintScriptRule (SCR rules)"
    if rule == "ComponentReferenceValidationRule":
        if side == "iglint" and ".scripts.extensionFunctions[" in path and "not found as sibling" in text:
            return "SCR305: extension functions are scripts and are checked (bug 1)"
        if side == "ign-lint" and (".scripts." in path or ".events." in path or "onChange" in path or "self." in text):
            return "not compared: ComponentReferenceValidationRule script part (SCR305)"
        if side == "iglint" and re.search(r"(view parameter|custom property) `[^`]*` is not defined", text):
            return "BIND005: custom/params existence"
        if side == "iglint" and _query_parameter(views.get(f["file"]), path):
            return "BIND005: references in query parameters and other binding expressions are checked"
        if not path.startswith("root"):
            return "BIND005: view-level bindings resolve from the view (bug 20)"
    if rule == "BadComponentReferenceRule":
        if side == "iglint" and "extensionFunctions" in path:
            return "whole run: extension functions are scripts (bug 1)"
        m = re.match(r"\w[\w ]* contains '(.*?)'", text)
        if side == "ign-lint" and m:
            expression = expression_binding(views.get(f["file"]), path)
            if expression is not None and python_in_expression(expression, m.group(1)):
                return "VIEW018: Python code in an expression binding is BIND001's finding (bug 18)"
    if rule == "NamePatternRule":
        m = re.search(r"Name '([^']*)'", text)
        if m and has_abbreviation(m.group(1)):
            return "NAME001: abbreviations are whole words (bug 6)"
        if "already in use" in text:
            return "NAME001: no self-collision message (bug 7)"
        if m and side == "ign-lint" and m.group(1).startswith("$"):
            return "NAME001: dataset encoding keys (`$`, `$columns`) are Ignition's names"
        if side == "iglint" and isinstance(resolve(views.get(f["file"]), path), (dict, list)):
            return "NAME001: keys whose value is an object or array are checked"
        if side == "iglint" and _propconfig_only(views.get(f["file"]), path):
            return "NAME001: propConfig-only component properties are checked"
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
        if side == "ign-lint" and view is not None and DYNAMIC_ACCESS.search(json.dumps(view)):
            return "VIEW015: a script reads the container dynamically, which counts as reading every property"
        if side == "ign-lint" and view is not None and LIBRARY_CALL.search(json.dumps(view)):
            return "VIEW015: a script calls a library function whose module evaluates computed strings or reaches a frame, so iglint reads every property it could reach (rounds 4-5 precision cost)"
        if side == "iglint":
            if view is not None and _mentioned_elsewhere(view, path):
                return "VIEW015: exact references; writes and look-alike names are not reads (bugs 21-23)"
            return None
    if rule == "ExcessiveContextDataRule" and side == "iglint":
        return "VIEW006: complete measurement (bug 24)"
    return None


def _mentioned_elsewhere(view, path):
    """True when the property's name occurs in the view other than as its
    own definition: ign-lint counts any such text (a write, a longer name
    that contains it, a same-named property elsewhere) as a use."""
    m = re.match(r"^(?:.*\.)?(custom|params)\.([^.\[]+)", path)
    if not m:
        return False
    text = json.dumps(view)
    return len(re.findall(rf"{re.escape(m.group(2))}", text)) > 1


def _query_parameter(view, path):
    """True when the last segment of `path` names a parameter of a query
    binding of the view (`{binding}.{parameter}` paths)."""
    name = path.rsplit(".", 1)[-1]
    found = []

    def walk(v):
        if isinstance(v, dict):
            if v.get("type") == "query" and isinstance(v.get("config"), dict):
                if name in (v["config"].get("parameters") or {}):
                    found.append(True)
            for x in v.values():
                walk(x)
        elif isinstance(v, list):
            for x in v:
                walk(x)

    walk(view)
    return bool(found)


def _propconfig_only(view, path):
    """True for `<component>.custom.<name>` that the component declares in
    its `propConfig` but not in its `custom` object."""
    head, sep, name = path.rpartition(".custom.")
    if not sep or "." in name or not head.startswith("root"):
        return False
    component = resolve(view, head)
    if not isinstance(component, dict):
        return False
    return name not in (component.get("custom") or {}) and f"custom.{name}" in (component.get("propConfig") or {})


DEFAULT_CATEGORY_MAPPING = {"F": "error", "E": "error", "W": "warning", "C": "warning", "R": "warning"}


def pylint_category_mapping(flags, corpus):
    """PylintScriptRule's `category_mapping` of the ign-lint configuration
    named by `--config` (relative to the corpus), else ign-lint's default."""
    for i, flag in enumerate(flags[:-1]):
        if flag != "--config":
            continue
        path = Path(flags[i + 1])
        if not path.is_absolute():
            path = corpus / path
        try:
            config = json.loads(path.read_text(encoding="utf-8"))
        except (OSError, ValueError):
            break
        rule = config.get("PylintScriptRule") if isinstance(config, dict) else None
        kwargs = (rule or {}).get("kwargs") if isinstance(rule, dict) else None
        mapping = (kwargs or {}).get("category_mapping") if isinstance(kwargs, dict) else None
        if isinstance(mapping, dict):
            return mapping
        break
    return DEFAULT_CATEGORY_MAPPING


def classify_severity(f, g, mapping):
    """Category for a matched pair (`f` from ign-lint, `g` from iglint) whose
    severities differ, or None when unexplained."""
    if f["rule"] == "PylintScriptRule" and g["category"] is None:
        # A native SCR rule takes the severity mapped for `E` (error-level
        # rules) or `W` (warning-level rules), not that of the pylint
        # category ign-lint grouped the message under.
        if g["severity"] in (mapping.get("E"), mapping.get("W")):
            return "SCR: native rules take the severity mapped for E or W"
    return None


def _debug_off_config(flags, out, corpus):
    """`flags` with every `--config PATH` pointing at an ign-lint
    configuration replaced by a copy in `out` that turns `debug` off and
    points every `debug_dir` into `out`. The real ign-lint stages scripts
    into `debug_dir` whatever `debug` says, so a config that names a
    `debug_dir` inside the corpus (the default `.ignition-lint/debug/`)
    writes into the corpus; this keeps all of that in the scratch
    directory. Configs without those keys are left untouched."""
    debug_dir = out / "ign-lint-debug"

    def sanitised(o):
        """A copy of `o` with `debug` off and `debug_dir` in `out`; returns
        (new, changed)."""
        changed = False
        if isinstance(o, dict):
            new = {}
            for k, v in o.items():
                if k == "debug" and v:
                    new[k], changed = False, True
                elif k == "debug_dir":
                    debug_dir.mkdir(parents=True, exist_ok=True)
                    new[k], changed = str(debug_dir), True
                else:
                    nv, c = sanitised(v)
                    new[k], changed = nv, changed or c
            return new, changed
        if isinstance(o, list):
            out_list = []
            for v in o:
                nv, c = sanitised(v)
                out_list.append(nv)
                changed = changed or c
            return out_list, changed
        return o, False

    cleaned = []
    i = 0
    while i < len(flags):
        cleaned.append(flags[i])
        if flags[i] == "--config" and i + 1 < len(flags):
            # ign-lint resolves a relative --config against the corpus (its
            # working directory), so resolve it there to read it.
            path = Path(flags[i + 1])
            if not path.is_absolute():
                path = corpus / path
            try:
                data = json.loads(path.read_text(encoding="utf-8"))
            except (OSError, ValueError):
                data = None
            new, changed = sanitised(data) if data is not None else (None, False)
            if changed:
                copy = out / f"config-nodebug-{path.name}"
                copy.write_text(json.dumps(new, indent=2), encoding="utf-8")
                cleaned.append(str(copy))
            else:
                cleaned.append(flags[i + 1])
            i += 2
            continue
        i += 1
    return cleaned


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
    out = Path(a.out or (BENCH_HOME / "parity-private" if private else REPO / "bench/results/parity" / corpus.name)).resolve()
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
    # Real ign-lint stages scripts into the `debug_dir` of its
    # configuration (default `.ignition-lint/debug/`, inside the corpus)
    # whatever `debug` says. Never write into the corpus: run with a
    # scratch copy of the configuration in --out, with `debug` off and
    # every `debug_dir` pointed into --out.
    resolved = _debug_off_config(resolved, out, corpus)
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
    pairs = []
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
            pairs.append((f, g))
        else:
            unmatched_left.append(f)
    rest_right = [f for fs in pool.values() for f in fs]
    pool2 = collections.defaultdict(list)
    for f in rest_right:
        pool2[key(f, True)].append(f)
    for f in unmatched_left:
        k = key(f, True)
        if pool2.get(k):
            pairs.append((f, pool2[k].pop()))
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
    mapping = pylint_category_mapping(resolved, corpus)
    severity_categories = collections.Counter()
    severity_diffs = []
    for f, g in pairs:
        if f["severity"] == g["severity"]:
            continue
        cat = classify_severity(f, g, mapping)
        if cat:
            severity_categories[cat] += 1
        else:
            severity_diffs.append({"ign-lint severity": f["severity"], "iglint severity": g["severity"], **f})
    print(f"\nfindings: ign-lint {len(left)}, iglint {len(right)}")
    print(f"{'rule':38} {'identical':>9} {'expected':>9} {'ign-lint-only':>13} {'iglint-only':>11}")
    for r in rules:
        s = stats[r]
        print(f"{r:38} {s['identical']:>9} {s['expected']:>9} {s['ign-lint-only']:>13} {s['iglint-only']:>11}")
    if stats and any(stats[r]["expected: wording"] for r in rules):
        categories["message wording (bugs 8, 18)"] += sum(stats[r]["expected: wording"] for r in rules)
    print(
        "\nnote: PylintScriptRule (SCR900) findings are outside this comparison: they count as\n"
        "expected ('not compared') and are compared by message code below. They differ by\n"
        "design: iglint drops ign-lint's combined-file header messages (C0103, W1405), does\n"
        "not report E0602 for names Ignition injects, and reports W0702 once, as SCR106."
    )
    print("\nexpected differences by category:")
    for c, n in categories.most_common():
        print(f"  {n:6}  {c}")
    unexplained = collections.Counter((d["side"], d["rule"], _shape(d)) for d in diffs)
    print("\nunexplained differences by side, rule and message shape:")
    for (side, rule, shape), n in unexplained.most_common():
        print(f"  {n:6}  {side:13} {rule:34} {shape if not private else ''}")
    # PylintScriptRule (SCR900): ign-lint lints one combined file per view,
    # iglint every script on its own, so lines and file-level messages
    # differ. Compare (file, node path, message code) multisets.
    def codes(findings):
        c = collections.Counter()
        for f in findings:
            m = re.search(r"\(([A-Z]\d{4})\)$", f["text"])
            if f["rule"] == "PylintScriptRule" and m:
                c[(f["file"], f["path"], m.group(1))] += 1
        return c

    lc, rc = codes(left), codes(right)
    if lc or rc:
        per_code = collections.defaultdict(collections.Counter)
        for k in set(lc) | set(rc):
            both = min(lc[k], rc[k])
            per_code[k[2]]["both"] += both
            per_code[k[2]]["ign-lint-only"] += lc[k] - both
            per_code[k[2]]["iglint-only"] += rc[k] - both
        print("\nPylintScriptRule / SCR900 by message code (file, node path, code):")
        print(f"  {'code':6} {'both':>6} {'ign-lint-only':>13} {'iglint-only':>11}")
        for code, c in sorted(per_code.items(), key=lambda kv: -sum(kv[1].values()))[:20]:
            print(f"  {code:6} {c['both']:>6} {c['ign-lint-only']:>13} {c['iglint-only']:>11}")
    differing = sum(severity_categories.values()) + len(severity_diffs)
    print(f"\nseverity of {len(pairs)} matched findings: {len(pairs) - differing} same, {differing} different")
    for c, n in severity_categories.most_common():
        print(f"  {n:6}  expected: {c}")
    unexplained_severity = collections.Counter(
        (d["rule"], d["ign-lint severity"], d["iglint severity"]) for d in severity_diffs
    )
    for (rule, sl, sr), n in unexplained_severity.most_common():
        print(f"  {n:6}  unexplained: {rule:34} ign-lint {sl}, iglint {sr}")
    (out / "differences.json").write_text(json.dumps(diffs, indent=1, ensure_ascii=False), encoding="utf-8")
    (out / "severity-differences.json").write_text(
        json.dumps(severity_diffs, indent=1, ensure_ascii=False), encoding="utf-8"
    )
    summary = {
        "findings": {"ign-lint": len(left), "iglint": len(right)},
        "identical": sum(stats[r]["identical"] for r in rules),
        "expected": sum(stats[r]["expected"] for r in rules),
        "expected by category": dict(categories),
        "unexplained": {
            "ign-lint-only": sum(stats[r]["ign-lint-only"] for r in rules),
            "iglint-only": sum(stats[r]["iglint-only"] for r in rules),
        },
        "severity": {
            "matched": len(pairs),
            "same": len(pairs) - differing,
            "expected by category": dict(severity_categories),
            "unexplained": len(severity_diffs),
        },
    }
    (out / "summary.json").write_text(json.dumps(summary, indent=1, ensure_ascii=False), encoding="utf-8")
    print(f"\nunexplained: {len(diffs)} (details: {out / 'differences.json'})")
    print(f"unexplained severity differences: {len(severity_diffs)} (details: {out / 'severity-differences.json'})")
    if a.list and not private:
        for d in diffs:
            print(f"{d['side']:13} {d['file']}  {d['rule']}: {d['raw']}")
        for d in severity_diffs:
            print(f"severity      {d['file']}  {d['rule']}: ign-lint {d['ign-lint severity']}, iglint {d['iglint severity']}: {d['raw']}")
    return 0


def _shape(d):
    """Message with names and numbers removed, for grouping."""
    t = re.sub(r"'[^']*'", "'…'", d["text"])
    t = re.sub(r"\{[^}]*\}", "{…}", t)
    t = re.sub(r"\d+", "N", t)
    return t[:90]


if __name__ == "__main__":
    sys.exit(main())
