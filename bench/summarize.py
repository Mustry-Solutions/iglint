#!/usr/bin/env python3
"""Summarise a bench/run.sh results directory as Markdown tables.

  bench/summarize.py RESULTS_DIR
"""

import json
import sys
from pathlib import Path

# ign-lint configurations that run pylint and report its messages. Batch
# mode is measured but excluded: ign-lint 0.6.4 prints its pylint messages
# as empty strings (see bench/README.md).
IGN_LINT_WORKING = ["ign-lint-default", "ign-lint-jobs1"]
ORDER = [
    "ign-lint-default",
    "ign-lint-jobs1",
    "ign-lint-batch",
    "ign-lint-batch-jobs1",
    "ign-lint-rules-only",
    "iglint-ign-lint-pylint",
    "iglint-ign-lint-pylint-1core",
    "iglint-ign-lint-rules-only",
    "iglint-ign-lint-rules-only-1core",
    "iglint-check-same-rules",
    "iglint-cold",
    "iglint-cold-1core",
    "iglint-cold-write",
    "iglint-warm",
    "iglint-incremental",
    "iglint-select-all",
    "iglint-migrated",
]
# Runs pinned to N CPUs: cpu<N>-<variant>.
PINNED = [
    ("ign-lint-default", "ign-lint, as shipped (the CI today)"),
    ("ign-lint-jobs1", "ign-lint `jobs=1`"),
    ("iglint-ign-lint-pylint", "`iglint ign-lint` (with pylint)"),
    ("iglint-migrated", "`iglint migrate` + baseline, `iglint check`"),
    ("iglint-cold", "`iglint check`, default rules"),
]


def cpu_limits(d):
    f = d / "cpu-limits.txt"
    return f.read_text().split() if f.exists() else []


def load(d):
    out = {}
    names = ORDER + [f"cpu{n}-{v}" for n in cpu_limits(d) for v, _ in PINNED]
    for name in names:
        f = d / f"{name}.json"
        if not f.exists():
            continue
        try:
            r = json.loads(f.read_text())["results"][0]
        except (ValueError, KeyError, IndexError):
            continue  # still running or failed
        if "summary" in r:  # hyperfine 2
            w = r["summary"]["time_wall_clock"]
            mem = r["summary"].get("memory_peak_resident")
            v = {"mean": w["mean"], "stddev": w.get("stddev") or 0.0, "min": w["min"], "runs": w["count"],
                 "rss": mem["max"] / 2**20 if mem else None}
        else:  # hyperfine 1
            mem = r.get("memory_usage_byte")
            v = {"mean": r["mean"], "stddev": r.get("stddev") or 0.0, "min": r["min"], "runs": len(r["times"]),
                 "rss": max(mem) / 2**20 if mem else None}
        rf = d / f"{name}.rss"
        if rf.exists() and rf.read_text().strip().isdigit():
            v["rss_time_v"] = int(rf.read_text()) / 1024
        out[name] = v
    return out


def fmt_t(s):
    return f"{s * 1000:.0f} ms" if s < 1 else f"{s:.2f} s"


def ratio(r, a, b):
    """`a / b` on minimum wall times (the least disturbed runs)."""
    if a in r and b in r:
        return r[a]["min"] / r[b]["min"]
    return None


def main():
    root = Path(sys.argv[1])
    corpora = sorted((p for p in root.iterdir() if (p / "views.txt").exists()),
                     key=lambda p: int((p / "views.txt").read_text()))
    headline = []
    for d in corpora:
        r = load(d)
        views = int((d / "views.txt").read_text())
        cpus = (d / "cpus.txt").read_text().strip() if (d / "cpus.txt").exists() else "?"
        working = {k: v for k, v in r.items() if k in IGN_LINT_WORKING}
        best = min(working, key=lambda k: working[k]["min"]) if working else None
        print(f"### {d.name} ({views} views, {cpus} CPUs)\n")
        print("| variant | min | mean ± σ | runs | peak RSS |")
        print("|---|---|---|---|---|")
        for k, v in r.items():
            rss = f"{v['rss']:.0f} MB" if v["rss"] else ""
            if v.get("rss_time_v"):
                rss += f" (time -v {v['rss_time_v']:.0f} MB)"
            mark = " (best working)" if k == best else ""
            print(f"| {k}{mark} | {fmt_t(v['min'])} | {fmt_t(v['mean'])} ± {fmt_t(v['stddev'])} | {v['runs']} | {rss} |")
        print()

        def x(a, b):
            q = ratio(r, a, b)
            return f"{q:.0f}x" if q and q >= 10 else (f"{q:.1f}x" if q else "")

        limits = cpu_limits(d)
        if limits:
            print(f"Pinned with `taskset` (minimum wall time; x = ign-lint as shipped / this):\n")
            print("| | " + " | ".join(f"{n} CPU{'s' if n != '1' else ''}" for n in limits) + " |")
            print("|---|" + "---|" * len(limits))
            for v, label in PINNED:
                cells = []
                for n in limits:
                    k = f"cpu{n}-{v}"
                    if k not in r:
                        cells.append("")
                        continue
                    q = ratio(r, f"cpu{n}-ign-lint-default", k)
                    cells.append(fmt_t(r[k]["min"]) + (f" ({q:.1f}x)" if q and not v.startswith("ign-lint-default") else ""))
                print(f"| {label} | " + " | ".join(cells) + " |")
            print()

        headline.append([
            d.name, str(views),
            f"{fmt_t(r[best]['min'])} ({best.removeprefix('ign-lint-')})" if best else "",
            x(best, "iglint-ign-lint-pylint") if best else "",
            x(best, "iglint-ign-lint-pylint-1core") if best else "",
            x("ign-lint-batch-jobs1", "iglint-ign-lint-pylint"),
            x("ign-lint-rules-only", "iglint-ign-lint-rules-only"),
            x("ign-lint-rules-only", "iglint-check-same-rules"),
            x(best, "iglint-cold") if best else "",
            x(best, "iglint-warm") if best else "",
        ])
    if headline:
        print("### Speedups (ratio of minimum wall times)\n")
        print("| corpus | views | ign-lint best working | with pylint: `iglint ign-lint` | same, 1 core | vs broken batch mode | rules only: `iglint ign-lint` | rules only: `iglint check` | native default rules, cold | warm |")
        print("|---|---|---|---|---|---|---|---|---|---|")
        for row in headline:
            print("| " + " | ".join(row) + " |")


if __name__ == "__main__":
    main()
