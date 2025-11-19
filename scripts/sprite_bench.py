#!/usr/bin/env python3
"""
Utility for collecting sprite benchmark baselines without piling up heavy artefacts.

Examples:
    python scripts/sprite_bench.py --label before_phase0 --runs 3
    python scripts/sprite_bench.py --bench-release --label bench_release --runs 1
"""

from __future__ import annotations

import argparse
import datetime
import json
import os
import shlex
import statistics as stats
import subprocess
import sys
from pathlib import Path
from typing import Dict, List, Optional, Tuple

REPO_ROOT = Path(__file__).resolve().parents[1]
DEFAULT_REPORT = REPO_ROOT / "target" / "animation_targets_report.json"
PERF_DIR = REPO_ROOT / "perf"


def build_command(args: argparse.Namespace) -> List[str]:
    cmd: List[str] = ["cargo", "test"]
    profile = args.profile.lower()
    if profile == "release":
        cmd.append("--release")
    elif profile not in ("dev", ""):
        cmd.extend(["--profile", profile])
    if args.features:
        cmd.extend(["--features", args.features])
    cmd.append(args.test)
    extra = shlex.split(args.test_args) if args.test_args else []
    if extra:
        cmd.append("--")
        cmd.extend(extra)
    return cmd


def run_once(cmd: List[str], env: Dict[str, str]) -> None:
    subprocess.run(cmd, check=True, cwd=REPO_ROOT, env=env)


def read_report(report_path: Path) -> Tuple[Dict[str, object], List[dict]]:
    if not report_path.exists():
        raise FileNotFoundError(f"Benchmark report not found: {report_path}")
    payload = json.loads(report_path.read_text(encoding="utf-8"))
    if isinstance(payload, dict):
        metadata = payload.get("metadata", {})
        cases = payload.get("cases", [])
    else:
        metadata = {}
        cases = payload
    return metadata, cases


def load_baseline(path: Path) -> Tuple[Dict[str, float], Dict[str, object]]:
    if not path.exists():
        raise FileNotFoundError(f"Baseline summary not found: {path}")
    payload = json.loads(path.read_text(encoding="utf-8"))
    mapping = {entry["label"]: entry.get("mean_ms", 0.0) for entry in payload.get("systems", [])}
    meta = {
        "path": str(path),
        "label": payload.get("label"),
        "commit": payload.get("commit"),
        "timestamp": payload.get("timestamp"),
    }
    return mapping, meta


def git_rev() -> str:
    return (
        subprocess.check_output(["git", "rev-parse", "HEAD"], cwd=REPO_ROOT)
        .decode("utf-8")
        .strip()
    )


def parse_args(argv: List[str]) -> argparse.Namespace:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--label", default="latest_sprite_bench", help="Base filename for perf artefacts")
    parser.add_argument("--runs", type=int, default=3, help="Number of harness invocations")
    parser.add_argument("--profile", default="release", help="Cargo profile (default: release)")
    parser.add_argument(
        "--bench-release",
        action="store_true",
        help="Shortcut for --profile bench-release (optimized release bench)",
    )
    parser.add_argument("--features", default="", help="Optional cargo features")
    parser.add_argument("--test", default="animation_targets_measure", help="Test target to invoke")
    parser.add_argument(
        "--test-args",
        default="--ignored --nocapture",
        help="Arguments passed after `--` to the cargo test invocation",
    )
    parser.add_argument("--report-path", default=str(DEFAULT_REPORT), help="Path to animation_targets_report.json")
    parser.add_argument("--count", type=int, default=10_000, help="ANIMATION_PROFILE_COUNT value")
    parser.add_argument("--steps", type=int, default=240, help="ANIMATION_PROFILE_STEPS value")
    parser.add_argument("--warmup", type=int, default=16, help="ANIMATION_PROFILE_WARMUP value")
    parser.add_argument("--dt", type=float, default=1.0 / 60.0, help="ANIMATION_PROFILE_DT value")
    parser.add_argument("--baseline", default="", help="Optional JSON summary to diff against")
    return parser.parse_args(argv)


def format_table(rows: List[List[str]]) -> str:
    if not rows:
        return ""
    widths = [0] * len(rows[0])
    for row in rows:
        for idx, cell in enumerate(row):
            widths[idx] = max(widths[idx], len(cell))
    formatted = []
    for row in rows:
        padded = "  ".join(cell.rjust(widths[idx]) for idx, cell in enumerate(row))
        formatted.append(padded)
    return "\n".join(formatted)


def main(argv: List[str]) -> int:
    args = parse_args(argv)
    if args.bench_release:
        args.profile = "bench-release"
    PERF_DIR.mkdir(exist_ok=True)

    env = os.environ.copy()
    env["ANIMATION_PROFILE_COUNT"] = str(args.count)
    env["ANIMATION_PROFILE_STEPS"] = str(args.steps)
    env["ANIMATION_PROFILE_WARMUP"] = str(args.warmup)
    env["ANIMATION_PROFILE_DT"] = f"{args.dt:.9f}"

    cmd = build_command(args)
    baseline_map: Dict[str, float] = {}
    baseline_meta: Optional[Dict[str, object]] = None
    has_baseline = bool(args.baseline)
    if has_baseline:
        baseline_path = Path(args.baseline)
        baseline_map, baseline_meta = load_baseline(baseline_path)
    report_path = Path(args.report_path)
    cases: Dict[str, Dict[str, object]] = {}

    print(f"[sprite_bench] running {args.runs} iteration(s): {' '.join(cmd)}")
    bench_meta: Optional[Dict[str, object]] = None
    for idx in range(1, args.runs + 1):
        print(f"[sprite_bench] run {idx}/{args.runs}")
        run_once(cmd, env)
        meta, report_entries = read_report(report_path)
        if meta and bench_meta is None:
            bench_meta = meta
        for entry in report_entries:
            label = entry["label"]
            slot = cases.setdefault(
                label,
                {
                    "units": entry.get("units"),
                    "count": entry.get("count"),
                    "budget": entry.get("budget_ms"),
                    "steps": entry.get("steps"),
                    "samples": entry.get("samples"),
                    "runs": [],
                },
            )
            slot["runs"].append(entry["summary"]["mean_step_ms"])

    timestamp = datetime.datetime.now().isoformat(timespec="seconds")
    commit = git_rev()
    env_keys = sorted(k for k in env if k.startswith("ANIMATION_PROFILE_"))
    env_lines = [f"{key}={env[key]}" for key in env_keys]

    header = ["system"] + [f"run{i}" for i in range(1, args.runs + 1)] + ["mean", "stddev"]
    if has_baseline:
        header.append("delta")
    header.append("budget")
    rows = [header]
    summary_payload = {
        "label": args.label,
        "timestamp": timestamp,
        "commit": commit,
        "command": cmd,
        "env": {key: env[key] for key in env_keys},
        "systems": [],
    }
    if has_baseline and baseline_meta:
        summary_payload["baseline"] = baseline_meta
    if bench_meta:
        summary_payload["animation_targets_metadata"] = bench_meta
    for label in sorted(cases.keys()):
        slot = cases[label]
        values: List[float] = slot["runs"]
        mean_val = stats.mean(values)
        std_val = stats.pstdev(values) if len(values) > 1 else 0.0
        budget = slot["budget"]
        delta_cell = ""
        delta_payload: Optional[float] = None
        if has_baseline:
            baseline_mean = baseline_map.get(label)
            if baseline_mean is None:
                delta_cell = "n/a"
            else:
                delta_payload = mean_val - baseline_mean
                delta_cell = f"{delta_payload:+.3f}"
        row = [label] + [f"{v:.3f}" for v in values] + [f"{mean_val:.3f}", f"{std_val:.3f}"]
        if has_baseline:
            row.append(delta_cell if delta_cell else "n/a")
        row.append(f"{budget:.3f}")
        rows.append(row)
        entry_payload = {
            "label": label,
            "units": slot["units"],
            "count": slot["count"],
            "budget_ms": budget,
            "runs": list(values),
            "mean_ms": mean_val,
            "stddev_ms": std_val,
        }
        if delta_payload is not None:
            entry_payload["delta_vs_baseline_ms"] = delta_payload
        summary_payload["systems"].append(entry_payload)

    summary_lines = [
        f"Sprite benchmark summary: {args.label}",
        f"Timestamp: {timestamp}",
        f"Commit: {commit}",
        f"Command: {' '.join(cmd)}",
        "Environment:",
    ]
    summary_lines.extend(f"  - {line}" for line in env_lines)
    if has_baseline and baseline_meta:
        summary_lines.append(
            "Baseline: {label} (commit {commit}) @ {path} [{timestamp}]".format(
                label=baseline_meta.get("label") or "n/a",
                commit=baseline_meta.get("commit") or "n/a",
                path=baseline_meta.get("path") or args.baseline,
                timestamp=baseline_meta.get("timestamp") or "n/a",
            )
        )
    if bench_meta:
        summary_lines.append("Animation targets metadata:")
        for key in (
            "warmup_frames",
            "measured_frames",
            "samples_per_case",
            "dt",
            "profile",
            "lto_mode",
            "target_cpu",
            "rustc_version",
        ):
            if key in bench_meta:
                summary_lines.append(f"  - {key}: {bench_meta[key]}")
    summary_lines.append("")
    summary_lines.append("Per-run mean_step_ms (ms):")
    summary_lines.append(format_table(rows))
    summary_text = "\n".join(summary_lines) + "\n"

    text_path = PERF_DIR / f"{args.label}.txt"
    json_path = PERF_DIR / f"{args.label}.json"
    text_path.write_text(summary_text, encoding="utf-8")
    json_path.write_text(json.dumps(summary_payload, indent=2), encoding="utf-8")
    print(f"[sprite_bench] wrote {text_path}")
    print(f"[sprite_bench] wrote {json_path}")
    return 0


if __name__ == "__main__":
    raise SystemExit(main(sys.argv[1:]))
