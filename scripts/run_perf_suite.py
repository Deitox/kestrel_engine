#!/usr/bin/env python3
"""
Run the sprite benchmark and capture harness in a single pass, emitting a consolidated summary.

Example:
    python scripts/run_perf_suite.py --label before_phase1 --runs 3 --sprite-baseline perf/before_phase0.json
"""

from __future__ import annotations

import argparse
import datetime as dt
import json
import shlex
import subprocess
import sys
from pathlib import Path
from typing import Dict, List, Optional

REPO_ROOT = Path(__file__).resolve().parents[1]
PERF_DIR = REPO_ROOT / "perf"
SPRITE_BENCH = REPO_ROOT / "scripts" / "sprite_bench.py"
CAPTURE_SCRIPT = REPO_ROOT / "scripts" / "capture_sprite_perf.py"


def parse_args() -> argparse.Namespace:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--label", default="perf_suite", help="Logical label for the combined run")
    parser.add_argument("--bench-label", default=None, help="Override label passed to sprite_bench.py")
    parser.add_argument("--capture-label", default=None, help="Override label passed to capture_sprite_perf.py")
    parser.add_argument("--runs", type=int, default=3, help="Number of iterations to pass to the harnesses")
    parser.add_argument("--count", type=int, default=10_000, help="ANIMATION_PROFILE_COUNT value")
    parser.add_argument("--steps", type=int, default=240, help="ANIMATION_PROFILE_STEPS value")
    parser.add_argument("--warmup", type=int, default=16, help="ANIMATION_PROFILE_WARMUP value")
    parser.add_argument("--dt", type=float, default=1.0 / 60.0, help="ANIMATION_PROFILE_DT value")
    parser.add_argument("--bench-profile", default="release", help="Cargo profile for sprite bench runs")
    parser.add_argument("--bench-features", default="", help="Optional cargo features for sprite bench")
    parser.add_argument("--bench-test", default="animation_targets_measure", help="Test target for sprite bench")
    parser.add_argument(
        "--bench-test-args",
        default="--ignored --nocapture",
        help="Args forwarded to sprite_bench.py --test-args",
    )
    parser.add_argument("--sprite-baseline", default="", help="Optional baseline JSON for sprite bench diffs")
    parser.add_argument("--report-path", default=None, help="Override animation_targets_report location")
    parser.add_argument("--skip-bench", action="store_true", help="Skip sprite_bench.py invocation")
    parser.add_argument("--skip-capture", action="store_true", help="Skip capture_sprite_perf.py invocation")
    parser.add_argument("--capture-extra", default="", help="Extra args appended to capture_sprite_perf.py")
    parser.add_argument(
        "--python",
        default=sys.executable or "python",
        help="Python interpreter used to invoke helper scripts (defaults to the current interpreter).",
    )
    return parser.parse_args()


def ensure_perf_dir() -> None:
    PERF_DIR.mkdir(exist_ok=True)


def shell_join(parts: List[str]) -> str:
    return " ".join(shlex.quote(part) for part in parts)


def load_json(path: Path) -> Dict[str, object]:
    with path.open("r", encoding="utf-8") as handle:
        return json.load(handle)


def run_sprite_bench(args: argparse.Namespace) -> Dict[str, object]:
    bench_label = args.bench_label or args.label
    cmd: List[str] = [
        args.python,
        str(SPRITE_BENCH),
        "--label",
        bench_label,
        "--runs",
        str(args.runs),
        "--count",
        str(args.count),
        "--steps",
        str(args.steps),
        "--warmup",
        str(args.warmup),
        "--dt",
        f"{args.dt:.9f}",
        "--profile",
        args.bench_profile,
        "--test",
        args.bench_test,
        "--test-args",
        args.bench_test_args,
    ]
    if args.bench_features:
        cmd.extend(["--features", args.bench_features])
    if args.sprite_baseline:
        cmd.extend(["--baseline", args.sprite_baseline])
    if args.report_path:
        cmd.extend(["--report-path", args.report_path])
    print(f"[perf_suite] running sprite_bench: {shell_join(cmd)}")
    subprocess.run(cmd, cwd=REPO_ROOT, check=True)
    json_path = PERF_DIR / f"{bench_label}.json"
    txt_path = PERF_DIR / f"{bench_label}.txt"
    if not json_path.exists():
        raise FileNotFoundError(f"sprite bench summary missing at {json_path}")
    payload = load_json(json_path)
    return {
        "label": bench_label,
        "command": shell_join(cmd),
        "json_path": str(json_path.relative_to(REPO_ROOT)),
        "txt_path": str(txt_path.relative_to(REPO_ROOT)) if txt_path.exists() else None,
        "summary": payload,
    }


def run_capture(args: argparse.Namespace, skip_bench_flag: bool) -> Dict[str, object]:
    capture_label = args.capture_label or args.label
    cmd: List[str] = [
        args.python,
        str(CAPTURE_SCRIPT),
        "--label",
        capture_label,
        "--runs",
        str(args.runs),
        "--count",
        str(args.count),
        "--steps",
        str(args.steps),
        "--warmup",
        str(args.warmup),
        "--dt",
        f"{args.dt:.9f}",
        "--bench-profile",
        args.bench_profile,
        "--bench-features",
        args.bench_features,
        "--bench-test",
        args.bench_test,
        "--bench-test-args",
        args.bench_test_args,
    ]
    if skip_bench_flag:
        cmd.append("--skip-bench")
    if args.capture_extra:
        cmd.extend(shlex.split(args.capture_extra))
    print(f"[perf_suite] running capture_sprite_perf: {shell_join(cmd)}")
    subprocess.run(cmd, cwd=REPO_ROOT, check=True)
    json_path = PERF_DIR / f"{capture_label}_capture.json"
    if not json_path.exists():
        raise FileNotFoundError(f"capture summary missing at {json_path}")
    payload = load_json(json_path)
    return {
        "label": capture_label,
        "command": shell_join(cmd),
        "json_path": str(json_path.relative_to(REPO_ROOT)),
        "summary": payload,
    }


def main() -> int:
    args = parse_args()
    ensure_perf_dir()
    bench_result: Optional[Dict[str, object]] = None
    capture_result: Optional[Dict[str, object]] = None

    if not args.skip_bench:
        bench_result = run_sprite_bench(args)
    else:
        print("[perf_suite] skipping sprite_bench.py (per flag)")

    if not args.skip_capture:
        capture_skip_bench = bench_result is not None
        capture_result = run_capture(args, skip_bench_flag=capture_skip_bench)
    else:
        print("[perf_suite] skipping capture_sprite_perf.py (per flag)")

    timestamp = dt.datetime.now().isoformat(timespec="seconds")
    suite_summary = {
        "label": args.label,
        "timestamp": timestamp,
        "sprite_bench": bench_result,
        "capture_sprite_perf": capture_result,
    }
    suite_path = PERF_DIR / f"{args.label}_suite.json"
    suite_path.write_text(json.dumps(suite_summary, indent=2), encoding="utf-8")
    print(f"[perf_suite] wrote {suite_path.relative_to(REPO_ROOT)}")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
