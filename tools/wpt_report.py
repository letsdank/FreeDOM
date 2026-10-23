#!/usr/bin/env python3

import json
import os
import subprocess
import sys
from datetime import datetime, timezone
from pathlib import Path

ROOT = Path(__file__).resolve().parents[1]
OUTPUT_DIRECTORY = ROOT / "artifacts" / "wpt"
LATEST_REPORT = OUTPUT_DIRECTORY / "latest.json"
HISTORY_REPORT = OUTPUT_DIRECTORY / "history.json"
CARGO = os.environ.get("CARGO", "cargo")
METRIC_PREFIX = "FREEDOM_WPT_METRIC "


def git_revision():
    try:
        return subprocess.check_output(
            ["git", "rev-parse", "HEAD"],
            cwd=ROOT,
            text=True,
        ).strip()
    except subprocess.CalledProcessError:
        return None


def empty_suite():
    return {
        "tests": 0,
        "passed": 0,
        "failed": 0,
    }


def load_history():
    if not HISTORY_REPORT.exists():
        return []

    try:
        history = json.loads(HISTORY_REPORT.read_text(encoding="utf-8"))
    except json.JSONDecodeError:
        return []

    return history if isinstance(history, list) else []


def print_process_output(result):
    if result.stdout:
        sys.stdout.write(result.stdout)

    if result.stderr:
        sys.stderr.write(result.stderr)


def extract_metrics(output):
    metrics = []

    for output_line in output.splitlines():
        line = output_line.strip()

        if not line.startswith(METRIC_PREFIX):
            continue

        try:
            metric = json.loads(line.removeprefix(METRIC_PREFIX))
        except json.JSONDecodeError:
            continue

        if not isinstance(metric, dict):
            continue

        if not isinstance(metric.get("suite"), str):
            continue

        if not isinstance(metric.get("kind"), str):
            continue

        if not all(
            isinstance(metric.get(field), int)
            for field in ("passed", "total", "failed")
        ):
            continue

        metrics.append(metric)

    return metrics


def list_wpt_tests():
    result = subprocess.run(
        [CARGO, "test", "--", "--list"],
        cwd=ROOT,
        text=True,
        capture_output=True,
    )

    if result.returncode != 0:
        print_process_output(result)
        return None

    tests = []

    for line in result.stdout.splitlines():
        name, separator, _ = line.partition(": test")

        if separator and name.startswith("wpt::"):
            tests.append(name)

    return sorted(tests)


def main():
    test_names = list_wpt_tests()

    if test_names is None:
        print("WPT report was not written because test discovery failed.", file=sys.stderr)
        return 1

    if not test_names:
        print("WPT report was not written because no WPT tests were found.", file=sys.stderr)
        return 1

    suites = {}
    failures = []
    measurements = {}

    for test_name in test_names:
        suite_name = test_name.split("::")[1]
        suite = suites.setdefault(suite_name, empty_suite())

        print(f"Running {test_name}")

        result = subprocess.run(
            [CARGO, "test", "--no-fail-fast", test_name, "--", "--exact", "--nocapture"],
            cwd=ROOT,
            text=True,
            capture_output=True,
        )

        for metric in extract_metrics(f"{result.stdout}\n{result.stderr}"):
            measurement_id = f"{metric['suite']}_{metric['kind']}"
            measurements[measurement_id] = metric

        suite["tests"] += 1

        if result.returncode == 0:
            suite["passed"] += 1
            continue

        suite["failed"] += 1
        failures.append(test_name)

        print_process_output(result)

    totals = empty_suite()

    for suite in suites.values():
        for field in totals:
            totals[field] += suite[field]

    report = {
        "schema_version": 2,
        "generated_at": datetime.now(timezone.utc)
        .replace(microsecond=0)
        .isoformat()
        .replace("+00:00", "Z"),
        "git_revision": git_revision(),
        "runner": "stable-cargo-per-test",
        "test_command_template": [
            CARGO,
            "test",
            "--no-fail-fast",
            "<test-name>",
            "--",
            "--exact",
        ],
        "exit_code": 0 if not failures else 1,
        "totals": totals,
        "suites": dict(sorted(suites.items())),
        "measurements": dict(sorted(measurements.items())),
        "failures": failures,
    }

    OUTPUT_DIRECTORY.mkdir(parents=True, exist_ok=True)

    LATEST_REPORT.write_text(
        json.dumps(report, indent=2, sort_keys=True) + "\n",
        encoding="utf-8",
    )

    history = load_history()
    history.append(report)

    HISTORY_REPORT.write_text(
        json.dumps(history, indent=2, sort_keys=True) + "\n",
        encoding="utf-8",
    )

    dashboard = subprocess.run(
        [sys.executable, str(ROOT / "tools" / "wpt_dashboard.py")],
        cwd=ROOT,
    )

    if dashboard.returncode != 0:
        return dashboard.returncode

    print(
        "WPT summary: "
        f"{totals['passed']}/{totals['tests']} passed, "
        f"{totals['failed']} failed"
    )

    for suite_name, suite in suites.items():
        print(
            f"  {suite_name}: "
            f"{suite['passed']}/{suite['tests']} passed, "
            f"{suite['failed']} failed"
        )

    return report["exit_code"]


if __name__ == "__main__":
    sys.exit(main())