#!/usr/bin/env python3

import argparse
import json
import os
import re
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
CAPTURE_HEADER = re.compile(r"^---- (.+) (stdout|stderr) ----$")
RUN_HEADER = re.compile(r"^running \d+ tests?$")
RUN_SUMMARY = re.compile(
    r"^test result: (ok|FAILED)\. (\d+) passed; (\d+) failed; (\d+) ignored; "
    r"\d+ measured; \d+ filtered out; finished in .+$"
)
MAX_BATCH_NAMES = 200
MAX_BATCH_BYTES = 16 * 1024


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
        "ignored": 0,
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

    if len(tests) != len(set(tests)):
        print(
            "WPT test discovery found duplicate test names; cannot report them safely.",
            file=sys.stderr,
        )
        return None

    return sorted(tests)


def parse_batch_output(output, test_names, exit_code):
    """Reconcile libtest's captured output with the complete discovery list."""
    expected = set(test_names)
    statuses = {}
    captured = {name: [] for name in test_names}
    in_harness = False
    reading_statuses = False
    current_output = None
    run_counts = None

    for line in output.splitlines():
        if line.strip().startswith(METRIC_PREFIX) and (
            not in_harness or reading_statuses or current_output is None
        ):
            raise ValueError("WPT metric was printed outside a captured test output")
        if not in_harness:
            if RUN_HEADER.fullmatch(line):
                in_harness = True
                reading_statuses = True
                run_counts = {"passed": 0, "failed": 0, "ignored": 0}
            continue

        summary = RUN_SUMMARY.fullmatch(line)
        if summary:
            actual = tuple(int(value) for value in summary.groups()[1:])
            expected_counts = tuple(
                run_counts[status] for status in ("passed", "failed", "ignored")
            )
            if actual != expected_counts or (
                (summary.group(1) == "FAILED") != (actual[1] > 0)
            ):
                raise ValueError("test harness summary does not match individual results")
            in_harness = False
            reading_statuses = False
            current_output = None
            continue

        if reading_statuses:
            if line in ("successes:", "failures:"):
                reading_statuses = False
                continue
            if not line.startswith("test "):
                continue
            name, separator, status = line[5:].rpartition(" ... ")
            if not separator or not name.startswith("wpt::"):
                continue
            if name not in expected:
                raise ValueError(f"unexpected WPT test result: {name}")
            if name in statuses:
                raise ValueError(f"duplicate WPT test result: {name}")
            if status == "ok":
                statuses[name] = "passed"
            elif status == "FAILED":
                statuses[name] = "failed"
            elif status == "ignored" or status.startswith("ignored, "):
                statuses[name] = "ignored"
            else:
                raise ValueError(f"unrecognized WPT test status: {name}: {status}")
            run_counts[statuses[name]] += 1
            continue

        header = CAPTURE_HEADER.fullmatch(line)
        if header:
            current_output = header.group(1) if header.group(1) in expected else None
        elif current_output is not None:
            captured[current_output].append(line)

    if in_harness:
        raise ValueError("test harness output ended before its summary")
    missing = expected - statuses.keys()
    if missing:
        raise ValueError(
            f"missing results for {len(missing)} WPT tests: {', '.join(sorted(missing))}"
        )
    failed = any(status == "failed" for status in statuses.values())
    if (
        exit_code not in (0, 101)
        or (exit_code == 0 and failed)
        or (exit_code != 0 and not failed)
    ):
        raise ValueError(f"Cargo exit status {exit_code} is inconsistent with the test results")

    measurements = {}
    for name in sorted(test_names):
        for metric in extract_metrics("\n".join(captured[name])):
            measurements[f"{metric['suite']}_{metric['kind']}"] = metric
    return statuses, measurements


def batches(test_names):
    """Keep command lines bounded while preserving discovery order."""
    batch = []
    size = 0
    for name in test_names:
        name_size = len(os.fsencode(name)) + 1
        if name_size > MAX_BATCH_BYTES:
            raise ValueError(f"WPT test name exceeds the command-line limit: {name}")
        if batch and (
            len(batch) == MAX_BATCH_NAMES or size + name_size > MAX_BATCH_BYTES
        ):
            yield batch
            batch = []
            size = 0
        batch.append(name)
        size += name_size
    if batch:
        yield batch


def main(argv=None):
    parser = argparse.ArgumentParser(description="Run and summarize local WPT tests.")
    parser.add_argument(
        "--isolated",
        action="store_true",
        help="run each test in a separate Cargo process for debugging shared state",
    )
    args = parser.parse_args(argv)
    test_names = list_wpt_tests()

    if test_names is None:
        print("WPT report was not written because test discovery failed.", file=sys.stderr)
        return 1

    if not test_names:
        print("WPT report was not written because no WPT tests were found.", file=sys.stderr)
        return 1

    # A Cargo filter avoids unrelated doctests. With --exact it is not a test name;
    # the discovered names appended below are the only actual matches.
    command = [
        CARGO,
        "test",
        "--no-fail-fast",
        "wpt::",
        "--",
        "--exact",
        "--format=pretty",
        "--show-output",
        "--test-threads=1",
        "--color=never",
    ]
    environment = os.environ.copy()
    environment.pop("RUST_TEST_NOCAPTURE", None)
    try:
        test_batches = list(batches(test_names))
        if args.isolated:
            test_batches = [[name] for batch in test_batches for name in batch]
    except ValueError as error:
        print(f"WPT report was not written: {error}.", file=sys.stderr)
        return 1
    statuses = {}
    measurements = {}
    print(
        f"Running {len(test_names)} WPT tests in {len(test_batches)} batch(es)",
        flush=True,
    )
    for batch in test_batches:
        result = subprocess.run(
            command + batch, cwd=ROOT, text=True, capture_output=True, env=environment
        )
        try:
            if "process didn't exit successfully" in result.stderr:
                raise ValueError("a test process or Cargo helper exited abnormally")
            if any(
                line.strip().startswith(METRIC_PREFIX)
                for line in result.stderr.splitlines()
            ):
                raise ValueError("WPT metric was printed outside a captured test output")
            batch_statuses, batch_measurements = parse_batch_output(
                result.stdout, batch, result.returncode
            )
        except ValueError as error:
            print_process_output(result)
            print(f"WPT report was not written: {error}.", file=sys.stderr)
            return 1
        statuses.update(batch_statuses)
        measurements.update(batch_measurements)
        if any(status == "failed" for status in batch_statuses.values()):
            print_process_output(result)

    suites = {}
    failures = []
    for test_name in test_names:
        suite_name = test_name.split("::")[1]
        suite = suites.setdefault(suite_name, empty_suite())
        status = statuses[test_name]
        suite["tests"] += 1
        suite[status] += 1
        if status == "failed":
            failures.append(test_name)

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
        "runner": "stable-cargo-isolated" if args.isolated else "stable-cargo-batch",
        "test_command_template": command + ["<test-name>..."],
        "batch_count": len(test_batches),
        "exit_code": 0 if not failures else 1,
        "totals": totals,
        "suites": dict(sorted(suites.items())),
        "measurements": dict(sorted(measurements.items())),
        "failures": failures,
        "test_results": [
            {"name": name, "status": statuses[name]} for name in test_names
        ],
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
        f"{totals['failed']} failed, {totals['ignored']} ignored"
    )

    for suite_name, suite in suites.items():
        print(
            f"  {suite_name}: "
            f"{suite['passed']}/{suite['tests']} passed, "
            f"{suite['failed']} failed, {suite['ignored']} ignored"
        )

    return report["exit_code"]


if __name__ == "__main__":
    sys.exit(main())
