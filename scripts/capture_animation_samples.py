#!/usr/bin/env python3
"""
Generate deterministic scene captures for the animation samples.

Usage:
    python scripts/capture_animation_samples.py [animation_showcase]
"""

from __future__ import annotations

import argparse
import subprocess
import sys
from pathlib import Path

REPO_ROOT = Path(__file__).resolve().parents[1]
SCENES = {
    "animation_showcase": Path("assets/scenes/animation_showcase.json"),
    "skeletal_showcase": Path("assets/scenes/skeletal_showcase.json"),
}
CAPTURE_DIR = REPO_ROOT / "artifacts" / "scene_captures"


def run_capture(scene_name: str) -> None:
    scene_path = REPO_ROOT / SCENES[scene_name]
    out_path = CAPTURE_DIR / f"{scene_name}_capture.json"
    CAPTURE_DIR.mkdir(parents=True, exist_ok=True)
    cmd = [
        "cargo",
        "run",
        "--bin",
        "scene_capture",
        "--",
        "--scene",
        str(scene_path),
        "--out",
        str(out_path),
    ]
    print(f"[scene_capture] {scene_name}: {scene_path} -> {out_path}")
    subprocess.run(cmd, check=True)


def main(argv: list[str]) -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument(
        "scene",
        nargs="?",
        choices=sorted(SCENES.keys()),
        help="Optional scene name to capture. Defaults to all scenes.",
    )
    args = parser.parse_args(argv)

    if args.scene:
        run_capture(args.scene)
    else:
        for name in sorted(SCENES.keys()):
            run_capture(name)
    return 0


if __name__ == "__main__":
    raise SystemExit(main(sys.argv[1:]))
