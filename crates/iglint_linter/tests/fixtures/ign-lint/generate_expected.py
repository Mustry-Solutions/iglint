"""Regenerate the ign-lint parity expectations in this directory.

Runs ign-lint (pinned: 0.6.4) in-process, through the same calls its CLI makes
for one file in --fix-dry-run mode (read_json_file, flatten_json,
get_model_statistics, LintEngine.process with a PathTranslator), so every
finding and every proposed fix is captured with its rule and severity.

Usage (from a directory that has no .config/.ignition-pylintrc above it, so
PylintScriptRule uses the rcfile bundled with ign-lint):

    python -I generate_expected.py <this directory>

Writes, for every <dir>/view.json under cases/ and probes/:
    expected.json           findings of the 'default' variant
    expected-variants.json  {variant: {"findings": [...], "fixes": [...]}}
"""

import hashlib
import json
import re
import sys
from importlib.metadata import version
from pathlib import Path

from ignition_lint.cli import create_rules_from_config
from ignition_lint.common.flatten_json import flatten_json, read_json_file
from ignition_lint.common.fix_operations import FixOperationType
from ignition_lint.common.path_translator import PathTranslator
from ignition_lint.linter import LintEngine
from ignition_lint.rules import RULES_MAP

EXPECTED_VERSION = "0.6.4"

# pylint names the combined temp file HHMMSS_pid<PID>_<random>.py; syntax-error
# messages embed that module name, so it is replaced by a stable token.
# old_value of SET_VALUE/DELETE_KEY is the value already at json_path in the
# input view; above this many characters of canonical JSON it is stored as a
# digest of json.dumps(value, sort_keys=True, ensure_ascii=False).
MAX_INLINE_VALUE = 2000

TEMP_MODULE_RE = re.compile(r"\b\d{6}_pid\d+_\w+\b")


def variant_config(name, config):
    """Every non-default variant enables only the rules it names."""
    if name == "default":
        return dict(config)
    full = {rule: {"enabled": False} for rule in RULES_MAP}
    for rule, rule_config in config.items():
        full[rule] = {"enabled": True, **rule_config}
    return full


def split_message(message):
    node_path, sep, _ = message.partition(": ")
    return node_path if sep else None


def collect_findings(rules, results):
    """Findings in rule registry order; errors before warnings within a rule."""
    findings = []
    for rule in rules:
        name = rule.__class__.__name__
        for severity, bucket in (("error", results.errors), ("warning", results.warnings)):
            for message in bucket.get(name, []):
                if not message.strip():
                    continue  # PylintScriptRule count placeholder; real entries below
                findings.append({
                    "rule": name,
                    "severity": severity,
                    "node_path": split_message(message),
                    "message": message,
                })
        for violation in getattr(rule, "pylint_violations", []):
            path = rule._format_script_path(violation.path)  # pylint: disable=protected-access
            findings.append({
                "rule": name,
                "severity": rule.category_mapping.get(violation.category, rule.severity),
                "node_path": path,
                "message": TEMP_MODULE_RE.sub(
                    "<tmpmodule>", f"{path}: Line {violation.line}: {violation.message} ({violation.code})"
                ),
                "code": violation.code,
                "line": violation.line,
            })
    return findings


def compact(value):
    """Large old values are replaced by a digest so expectations stay small."""
    text = json.dumps(value, sort_keys=True, ensure_ascii=False)
    if len(text) <= MAX_INLINE_VALUE:
        return value
    return {"$sha256": hashlib.sha256(text.encode("utf-8")).hexdigest(), "$json_length": len(text)}


def serialize_fix(fix):
    operations = []
    for op in fix.operations:
        item = {"op": op.operation.value, "json_path": op.json_path, "description": op.description}
        if op.operation == FixOperationType.STRING_REPLACE:
            item.update(old_substring=op.old_substring, new_substring=op.new_substring)
        elif op.operation == FixOperationType.SET_VALUE:
            item.update(old_value=compact(op.old_value), new_value=op.new_value)
        else:
            item.update(old_value=compact(op.old_value))
        operations.append(item)
    return {
        "rule": fix.rule_name,
        "is_safe": fix.is_safe,
        "safety_notes": fix.safety_notes,
        "description": fix.description,
        "violation_message": fix.violation_message,
        "operations": operations,
    }


def run_variant(view_path, source_name, config):
    rules, statuses = create_rules_from_config(config)
    errored = [s for s in statuses if s["state"] == "error"]
    if errored:
        raise SystemExit(f"rule creation failed: {errored}")
    engine = LintEngine(rules)
    json_data = read_json_file(view_path)
    flattened = flatten_json(json_data)
    engine.get_model_statistics(flattened)
    translator = PathTranslator(json_data)
    results = engine.process(
        flattened, source_file_path=source_name, json_data=json_data, path_translator=translator
    )
    findings = collect_findings(rules, results)
    fixes = [serialize_fix(fix) for fix in results.fixes]
    return findings, fixes


def dump(path, data):
    with open(path, "w", encoding="utf-8", newline="\n") as handle:
        json.dump(data, handle, indent=2, ensure_ascii=False)
        handle.write("\n")


def main():
    if version("ign-lint") != EXPECTED_VERSION:
        raise SystemExit(f"need ign-lint {EXPECTED_VERSION}, found {version('ign-lint')}")
    root = Path(sys.argv[1]).resolve()
    with open(root / "variants.json", encoding="utf-8") as handle:
        raw = json.load(handle)
    variants = {k: v for k, v in raw.items() if not k.startswith("_")}
    excluded = raw.get("_exclude", {})
    views = sorted(p for sub in ("cases", "probes") for p in (root / sub).glob("*/view.json"))
    for view_path in views:
        source_name = view_path.relative_to(root).as_posix()
        per_variant = {}
        for name, config in variants.items():
            if view_path.parent.relative_to(root).as_posix() in excluded.get(name, []):
                continue
            findings, fixes = run_variant(view_path, source_name, variant_config(name, config))
            per_variant[name] = {"findings": findings, "fixes": fixes}
        dump(view_path.parent / "expected.json", per_variant["default"]["findings"])
        dump(view_path.parent / "expected-variants.json", per_variant)
        counts = {k: len(v["findings"]) for k, v in per_variant.items() if v["findings"]}
        print(source_name, counts, file=sys.stderr)


if __name__ == "__main__":
    main()
