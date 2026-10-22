#!/usr/bin/env python3
"""Summarise a bench/run.sh results directory as Markdown tables.

  bench/summarize.py RESULTS_DIR
"""

import json
import sys
from pathlib import Path

IGN_LINT_FULL = ["ign-lint-default", "ign-lint-jobs1", "ign-lint-batch", "ign-lint-batch-jobs1"]
ORDER = IGN_LINT_FULL + [
    "ign-lint-rules-only",
    "iglint-ign-lint-mode",
    "iglint-cold",
    "iglint-cold-1thread",
    "iglint-cold-write",
    "iglint-warm",
    "iglint-incremental",
    "iglint-select-all",
]


def load(d):
    out = {}
    for name in ORDER:
        f = d / f"{name}.json"
        if not f.exists():
            continue
        r = json.loads(f.read_text())["results"][0]
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
    return f"{s * 1000:.1f} ms" if s < 1 else f"{s:.2f} s"


def main():
    root = Path(sys.argv[1])
    corpora = sorted((p for p in root.iterdir() if (p / "views.txt").exists()), key=lambda p: int((p / "views.txt").read_text()))
    headline = []
    for d in corpora:
        r = load(d)
        views = int((d / "views.txt").read_text())
        full = {k: v for k, v in r.items() if k in IGN_LINT_FULL}
        best_name = min(full, key=lambda k: full[k]["mean"]) if full else None
        best = full[best_name]["mean"] if best_name else None
        print(f"### {d.name} ({views} views)\n")
        print("| variant | mean ± σ | min | runs | peak RSS | ign-lint best / this |")
        print("|---|---|---|---|---|---|")
        for k, v in r.items():
            sp = f"{best / v['mean']:.0f}x" if best and k.startswith("iglint") else ""
            rss = f"{v['rss']:.0f} MB" if v["rss"] else ""
            if v.get("rss_time_v"):
                rss += f" (time -v: {v['rss_time_v']:.0f} MB)"
            mark = " (best)" if k == best_name else ""
            print(f"| {k}{mark} | {fmt_t(v['mean'])} ± {fmt_t(v['stddev'])} | {fmt_t(v['min'])} | {v['runs']} | {rss} | {sp} |")
        print()
        if best and "iglint-cold" in r:
            row = [d.name, str(views), f"{fmt_t(best)} ({best_name.removeprefix('ign-lint-')})"]
            for k in ("iglint-cold", "iglint-warm", "iglint-incremental", "iglint-select-all"):
                row.append(f"{fmt_t(r[k]['mean'])} ({best / r[k]['mean']:.0f}x)" if k in r else "")
            if "ign-lint-rules-only" in r and "iglint-ign-lint-mode" in r:
                a, b = r["ign-lint-rules-only"]["mean"], r["iglint-ign-lint-mode"]["mean"]
                row.append(f"{fmt_t(a)} vs {fmt_t(b)} ({a / b:.0f}x)")
            headline.append(row)
    if headline:
        print("### Speedup summary\n")
        print("| corpus | views | ign-lint best | iglint cold | warm | incremental | --select ALL | rules only: ign-lint vs `iglint ign-lint` |")
        print("|---|---|---|---|---|---|---|---|")
        for row in headline:
            print("| " + " | ".join(row) + " |")


if __name__ == "__main__":
    main()
