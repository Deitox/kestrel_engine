#!/usr/bin/env python3
"""
Run both the sprite benchmark sweeps and the anim_stats profiling harness, then
drop lightweight artefacts under `perf/` for easy comparison.

Typical usage:

    python scripts/capture_sprite_perf.py --label after_phase1 --runs 3
"""

from __future__ import annotations

import argparse
import datetime as dt
import json
import os
import re
import shlex
import subprocess
import sys
from pathlib import Path
from typing import Dict, List, Optional, Sequence

REPO_ROOT = Path(__file__).resolve().parents[1]
PERF_DIR = REPO_ROOT / "perf"
SPRITE_BENCH = REPO_ROOT / "scripts" / "sprite_bench.py"

PROFILE_CMD = [
    "cargo",
    "test",
    "--release",
    "--features",
    "anim_stats",
    "--test",
    "animation_profile",
    "animation_profile_snapshot",
    "--",
    "--ignored",
    "--exact",
    "--nocapture",
]

STEP_STATS_RE = re.compile(
    r"\[animation_profile\] sys_drive per-step stats: "
    r"mean=(?P<mean>[0-9.]+) ms p95=(?P<p95>[0-9.]+) ms max=(?P<max>[0-9.]+) ms "
    r"steady_mean=(?P<steady>[0-9.]+) ms steady_samples=(?P<steady_samples>\d+) "
    r"spike_mean=(?P<spike>[0-9.]+) ms spike_samples=(?P<spike_samples>\d+)"
)

SPRITE_TOTALS_RE = re.compile(
    r"\[animation_profile\] anim_stats sprite totals: "
    r"fast_loop=(?P<fast_loop>\d+) event=(?P<event>\d+) plain=(?P<plain>\d+) "
    r"bsearch=(?P<bsearch>\d+) fast_bucket=(?P<fast_bucket>\d+) "
    r"general_bucket=(?P<general_bucket>\d+) applies=(?P<applies>\d+)"
)

SPRITE_BUCKET_AVG_RE = re.compile(
    r"\[animation_profile\] anim_stats sprite bucket avg: "
    r"fast=(?P<fast>[0-9.]+) entities/frame general=(?P<general>[0-9.]+) entities/frame"
)

TOP_STEP_RE = re.compile(
    r"\[animation_profile\]\s+step\s+(?P<index>\d+)\s+->\s+(?P<value>[0-9.]+) ms"
)

TOP_MIX_RE = re.compile(
    r"\[animation_profile\]\s+step\s+(?P<index>\d+)\s+->\s+(?P<value>[0-9.]+) ms \| "
    r"sprite\(fast=(?P<fast>\d+) event=(?P<event>\d+) plain=(?P<plain>\d+) "
    r"bsearch=(?P<bsearch>\d+) fast_bucket=(?P<fast_bucket>\d+) "
    r"general_bucket=(?P<general_bucket>\d+) applies=(?P<applies>\d+)\) "
    r"transform\(adv=(?P<adv>\d+) zero=(?P<zero>\d+) skipped=(?P<skipped>\d+) "
    r"loop_resume=(?P<loop_resume>\d+) zero_duration=(?P<zero_duration>\d+) "
    r"fast=(?P<fast_path>\d+) slow=(?P<slow_path>\d+)\) "
    r"time_ns\(adv=(?P<adv_ns>\d+) sample=(?P<sample_ns>\d+) apply=(?P<apply_ns>\d+)\)"
)


def parse_args(argv: Sequence[str]) -> argparse.Namespace:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--label", required=True, help="Base name for the perf artefacts (txt/json/log)")
    parser.add_argument("--runs", type=int, default=3, help="Number of sprite bench iterations (default: 3)")
    parser.add_argument("--count", type=int, default=10_000, help="ANIMATION_PROFILE_COUNT value")
    parser.add_argument("--steps", type=int, default=240, help="ANIMATION_PROFILE_STEPS value")
    parser.add_argument("--warmup", type=int, default=16, help="ANIMATION_PROFILE_WARMUP value")
    parser.add_argument("--dt", type=float, default=1.0 / 60.0, help="ANIMATION_PROFILE_DT value")
    parser.add_argument("--bench-profile", default="release", help="Cargo profile for sprite bench (default: release)")
    parser.add_argument("--bench-features", default="", help="Extra features for the sprite bench run")
    parser.add_argument("--bench-test", default="animation_targets_measure", help="Test target to pass to sprite_bench.py")
    parser.add_argument("--bench-test-args", default="--ignored --nocapture", help="Args passed after `--` to cargo test (sprite bench)")
    parser.add_argument("--profile-extra", default="", help="Extra args appended after `--` for the anim_stats harness")
    parser.add_argument("--skip-bench", action="store_true", help="Skip the sprite benchmark step")
    parser.add_argument("--skip-profile", action="store_true", help="Skip the anim_stats profiling step")
    parser.add_argument(
        "--python-cmd",
        default=sys.executable or "python",
        help="Python executable to use when invoking helper scripts (defaults to the current interpreter).",
    )
    return parser.parse_args(argv)


def run_subprocess(cmd: Sequence[str], *, env: Dict[str, str]) -> str:
    proc = subprocess.run(
        cmd,
        cwd=REPO_ROOT,
        env=env,
        text=True,
        capture_output=True,
        check=False,
    )
    if proc.returncode != 0:
        raise RuntimeError(
            f"Command {' '.join(cmd)} failed with exit code {proc.returncode}:\n{proc.stdout}\n{proc.stderr}"
        )
    if proc.stderr:
        # Preserve stderr alongside stdout for debugging; append to stdout so it's in the log too.
        return proc.stdout + ("\n" + proc.stderr if proc.stderr else "")
    return proc.stdout


def ensure_perf_dir() -> None:
    PERF_DIR.mkdir(exist_ok=True)


def build_env(args: argparse.Namespace) -> Dict[str, str]:
    env = os.environ.copy()
    env["ANIMATION_PROFILE_COUNT"] = str(args.count)
    env["ANIMATION_PROFILE_STEPS"] = str(args.steps)
    env["ANIMATION_PROFILE_WARMUP"] = str(args.warmup)
    env["ANIMATION_PROFILE_DT"] = f"{args.dt:.9f}"
    return env


def run_sprite_bench(args: argparse.Namespace, env: Dict[str, str]) -> List[str]:
    python_candidates: List[str] = []
    if args.python_cmd:
        python_candidates.append(args.python_cmd)
    if sys.executable and sys.executable not in python_candidates:
        python_candidates.append(sys.executable)
    if "python" not in python_candidates:
        python_candidates.append("python")

    base_args = ["--label", args.label, "--runs", str(args.runs)]
    if args.bench_profile:
        base_args.extend(["--profile", args.bench_profile])
    if args.bench_features:
        base_args.extend(["--features", args.bench_features])
    if args.bench_test:
        base_args.extend(["--test", args.bench_test])
    if args.bench_test_args:
        base_args.extend(["--test-args", args.bench_test_args])
    # propagate env knobs
    base_args.extend(["--count", str(args.count)])
    base_args.extend(["--steps", str(args.steps)])
    base_args.extend(["--warmup", str(args.warmup)])
    base_args.extend(["--dt", f"{args.dt:.9f}"])

    last_err: Optional[Exception] = None
    for candidate in python_candidates:
        cmd: List[str] = [candidate, str(SPRITE_BENCH), *base_args]
        try:
            subprocess.run(cmd, cwd=REPO_ROOT, env=env, check=True)
            return cmd
        except OSError as err:
            last_err = err
        except subprocess.CalledProcessError:
            raise

    raise RuntimeError(
        f"Failed to invoke {SPRITE_BENCH} with python executables {python_candidates}: {last_err}"
    )


def parse_profile_output(stdout: str) -> Dict[str, object]:
    data: Dict[str, object] = {}
    match = STEP_STATS_RE.search(stdout)
    if match:
        data["per_step_stats"] = {k: float(v) if "samples" not in k else int(v) for k, v in match.groupdict().items()}
    match = SPRITE_TOTALS_RE.search(stdout)
    if match:
        data["sprite_totals"] = {k: int(v) for k, v in match.groupdict().items()}
    match = SPRITE_BUCKET_AVG_RE.search(stdout)
    if match:
        data["sprite_bucket_avg"] = {k: float(v) for k, v in match.groupdict().items()}

    top_steps: List[Dict[str, float]] = []
    for m in TOP_STEP_RE.finditer(stdout):
        index = int(m.group("index"))
        value = float(m.group("value"))
        top_steps.append({"step": index, "ms": value})
    if top_steps:
        data["top_steps"] = top_steps

    top_mix: List[Dict[str, object]] = []
    for m in TOP_MIX_RE.finditer(stdout):
        entry = {"step": int(m.group("index")), "ms": float(m.group("value"))}
        for key in [
            "fast",
            "event",
            "plain",
            "bsearch",
            "fast_bucket",
            "general_bucket",
            "applies",
            "adv",
            "zero",
            "skipped",
            "loop_resume",
            "zero_duration",
            "fast_path",
            "slow_path",
        ]:
            entry[key] = int(m.group(key))
        for key in ["adv_ns", "sample_ns", "apply_ns"]:
            entry[key] = int(m.group(key))
        top_mix.append(entry)
    if top_mix:
        data["top_step_mix"] = top_mix
    return data


def run_animation_profile(args: argparse.Namespace, env: Dict[str, str]) -> Dict[str, object]:
    cmd = PROFILE_CMD.copy()
    if args.profile_extra:
        extra = shlex.split(args.profile_extra)
        if extra:
            cmd.extend(extra)
    stdout = run_subprocess(cmd, env=env)
    log_path = PERF_DIR / f"{args.label}_profile.log"
    log_path.write_text(stdout, encoding="utf-8")

    parsed = parse_profile_output(stdout)
    parsed["log"] = log_path.name
    return parsed


def main(argv: Sequence[str]) -> int:
    args = parse_args(argv)
    if args.skip_bench and args.skip_profile:
        raise SystemExit("Nothing to do: both bench and profile steps are disabled.")

    ensure_perf_dir()
    env = build_env(args)
    timestamp = dt.datetime.now().isoformat(timespec="seconds")
    commit = (
        subprocess.check_output(["git", "rev-parse", "HEAD"], cwd=REPO_ROOT)
        .decode("utf-8")
        .strip()
    )

    env_summary = {k: env[k] for k in sorted(env) if k.startswith("ANIMATION_PROFILE_")}

    bench_cmd: Optional[List[str]] = None
    if not args.skip_bench:
        bench_cmd = run_sprite_bench(args, env)

    profile_data: Optional[Dict[str, object]] = None
    if not args.skip_profile:
        profile_data = run_animation_profile(args, env)

    summary: Dict[str, object] = {"label": args.label, "timestamp": timestamp, "commit": commit}
    if bench_cmd is not None:
        summary["sprite_bench"] = {"runs": args.runs, "command": " ".join(bench_cmd), "env": env_summary}
    else:
        summary["sprite_bench"] = None
    if profile_data is not None:
        summary["animation_profile"] = {
            "command": " ".join(PROFILE_CMD + (shlex.split(args.profile_extra) if args.profile_extra else [])),
            "env": env_summary,
            "stats": profile_data,
        }
    else:
        summary["animation_profile"] = None

    json_path = PERF_DIR / f"{args.label}_capture.json"
    json_path.write_text(json.dumps(summary, indent=2), encoding="utf-8")
    print(f"[capture] wrote {json_path}")
    if profile_data is not None:
        print(f"[capture] profile stats: {profile_data.get('per_step_stats', {})}")
    return 0


if __name__ == "__main__":
    raise SystemExit(main(sys.argv[1:]))
