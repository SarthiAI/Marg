#!/usr/bin/env python3
"""Marg benchmark report generator.

Walks a results directory written by the bench runner, collates every
scenario's `summary.json` (or `summary.md`), and emits two artefacts:

  1. `<results-dir>/REPORT.md` -- a single human-readable cross-scenario
     pass / fail table for the run.
  2. The repo's `marg/BENCHMARKS.md` (rendered in place) with the result
     column for each scenario id updated to reflect the latest run.

Scenario directories are expected to look like:

    <results-dir>/manifest.json
    <results-dir>/<ID>-<name>/summary.json
    <results-dir>/<ID>-<name>/raw.json
    <results-dir>/<ID>-<name>/metrics.prom

`summary.json` (written by each scenario's runner script) carries the
machine-readable result:

    {
      "id": "T01",
      "name": "single-instance-passthrough",
      "rig": "single-node-prod",
      "gate": ">= 50000 req/s, p99 < 50ms for 10m",
      "passed": true,
      "headline": "52340 req/s sustained, p99 41ms",
      "notes": "first attempt after migration to redis hot"
    }

If `summary.json` is missing, the scenario is reported as "pending".

Usage:
    bench/report.py bench/results/2026-05-22-abcdef1
"""

from __future__ import annotations

import argparse
import json
import re
import sys
from pathlib import Path

REPO_ROOT = Path(__file__).resolve().parent.parent
BENCHMARKS_MD = REPO_ROOT / "BENCHMARKS.md"


def load_summary(scenario_dir: Path) -> dict | None:
    summary_path = scenario_dir / "summary.json"
    if not summary_path.exists():
        return None
    try:
        return json.loads(summary_path.read_text())
    except json.JSONDecodeError as e:
        print(f"warn: {summary_path} not valid JSON: {e}", file=sys.stderr)
        return None


def load_manifest(results_dir: Path) -> dict:
    manifest_path = results_dir / "manifest.json"
    if not manifest_path.exists():
        return {}
    try:
        return json.loads(manifest_path.read_text())
    except json.JSONDecodeError as e:
        print(f"warn: {manifest_path} not valid JSON: {e}", file=sys.stderr)
        return {}


def discover_scenarios(results_dir: Path) -> list[tuple[str, dict | None]]:
    scenarios: list[tuple[str, dict | None]] = []
    for child in sorted(results_dir.iterdir()):
        if not child.is_dir():
            continue
        m = re.match(r"^([A-Z]\d{2,3})-(.+)$", child.name)
        if not m:
            continue
        sid = m.group(1)
        summary = load_summary(child)
        scenarios.append((sid, summary))
    return scenarios


def render_run_report(
    results_dir: Path,
    manifest: dict,
    scenarios: list[tuple[str, dict | None]],
) -> str:
    lines: list[str] = []
    lines.append(f"# Bench run report: {results_dir.name}")
    lines.append("")
    if manifest:
        lines.append("## Manifest")
        lines.append("")
        for k, v in manifest.items():
            lines.append(f"- **{k}**: {v}")
        lines.append("")
    lines.append("## Scenarios")
    lines.append("")
    lines.append("| ID | Name | Rig | Gate | Result | Headline |")
    lines.append("|---|---|---|---|---|---|")
    for sid, summary in scenarios:
        if summary is None:
            lines.append(f"| {sid} | (no summary.json) | n/a | n/a | pending | n/a |")
            continue
        name = summary.get("name", "")
        rig = summary.get("rig", "n/a")
        gate = summary.get("gate", "n/a")
        passed = summary.get("passed")
        if passed is True:
            result = "pass"
        elif passed is False:
            result = "FAIL"
        else:
            result = "pending"
        headline = summary.get("headline", "")
        lines.append(f"| {sid} | {name} | {rig} | {gate} | {result} | {headline} |")
    lines.append("")
    return "\n".join(lines)


def update_benchmarks_md(scenarios: list[tuple[str, dict | None]]) -> None:
    if not BENCHMARKS_MD.exists():
        print(
            f"warn: {BENCHMARKS_MD} not present, skipping in-place update",
            file=sys.stderr,
        )
        return

    text = BENCHMARKS_MD.read_text()
    by_id: dict[str, dict | None] = {sid: summary for sid, summary in scenarios}

    def replace_row(match: re.Match[str]) -> str:
        prefix = match.group(1)
        sid = match.group(2)
        middle = match.group(3)
        old_result = match.group(4).strip()
        suffix = match.group(5)
        summary = by_id.get(sid)
        if summary is None:
            return match.group(0)
        passed = summary.get("passed")
        if passed is True:
            new_result = "pass"
        elif passed is False:
            new_result = "FAIL"
        else:
            new_result = "pending"
        if summary.get("headline"):
            new_result = f"{new_result}: {summary['headline']}"
        if new_result == old_result:
            return match.group(0)
        return f"{prefix}{sid}{middle}{new_result}{suffix}"

    pattern = re.compile(r"(\|\s*)([A-Z]\d{2,3})(\s*\|[^|\n]*\|[^|\n]*\|[^|\n]*\|[^|\n]*\|\s*)([^|\n]*?)(\s*\|)")
    new_text = pattern.sub(replace_row, text)
    if new_text != text:
        BENCHMARKS_MD.write_text(new_text)
        print(f"updated {BENCHMARKS_MD}")
    else:
        print("BENCHMARKS.md already current, no changes")


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("results_dir", help="path to bench/results/<run-id>")
    parser.add_argument(
        "--no-update",
        action="store_true",
        help="only print the run report, do not modify BENCHMARKS.md",
    )
    args = parser.parse_args()
    results_dir = Path(args.results_dir).resolve()
    if not results_dir.is_dir():
        print(f"error: {results_dir} is not a directory", file=sys.stderr)
        return 2

    manifest = load_manifest(results_dir)
    scenarios = discover_scenarios(results_dir)
    if not scenarios:
        print(f"warn: no scenario subdirectories found under {results_dir}", file=sys.stderr)

    report = render_run_report(results_dir, manifest, scenarios)
    report_path = results_dir / "REPORT.md"
    report_path.write_text(report)
    print(f"wrote {report_path}")

    if not args.no_update:
        update_benchmarks_md(scenarios)
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
