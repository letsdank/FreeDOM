import contextlib
import io
import tempfile
import unittest
from pathlib import Path
from unittest.mock import Mock, patch

import wpt_dashboard
import wpt_report


class BatchOutputTests(unittest.TestCase):
    names = ["wpt::css::pass", "wpt::url::fail", "wpt::svg::skip"]
    output = """running 3 tests
test wpt::css::pass ... ok
test wpt::url::fail ... FAILED
test wpt::svg::skip ... ignored, needs a feature

successes:

---- wpt::css::pass stdout ----
FREEDOM_WPT_METRIC {"suite":"url","kind":"cases","passed":3,"total":4,"failed":1}

failures:

---- wpt::url::fail stdout ----
assertion failed

failures:
    wpt::url::fail

test result: FAILED. 1 passed; 1 failed; 1 ignored; 0 measured; 0 filtered out; finished in 0.00s
"""

    def test_collects_individual_results_and_captured_metrics(self):
        statuses, measurements = wpt_report.parse_batch_output(self.output, self.names, 101)
        self.assertEqual(
            statuses,
            {
                "wpt::css::pass": "passed",
                "wpt::url::fail": "failed",
                "wpt::svg::skip": "ignored",
            },
        )
        self.assertEqual(measurements["url_cases"]["passed"], 3)

    def test_rejects_incomplete_duplicate_or_inconsistent_results(self):
        cases = [
            (
                self.output.replace("test wpt::svg::skip ... ignored, needs a feature\n", ""),
                101,
            ),
            (
                self.output.replace(
                    "test wpt::css::pass ... ok\n",
                    "test wpt::css::pass ... ok\ntest wpt::css::pass ... ok\n",
                ),
                101,
            ),
            (self.output, 0),
            (self.output, -9),
            (self.output.rsplit("test result:", 1)[0], 101),
            (
                self.output.replace("test wpt::css::pass ... ok", "test wpt::css::pass ... UNKNOWN"),
                101,
            ),
            (self.output.replace("1 passed; 1 failed", "2 passed; 0 failed"), 101),
        ]
        for output, code in cases:
            with self.subTest(code=code, output=output[:80]):
                with self.assertRaises(ValueError):
                    wpt_report.parse_batch_output(output, self.names, code)

    def test_does_not_silently_drop_uncaptured_metrics(self):
        output = self.output.replace(
            "test wpt::css::pass ... ok",
            'FREEDOM_WPT_METRIC {"suite":"url"}\ntest wpt::css::pass ... ok',
        )
        with self.assertRaisesRegex(ValueError, "outside a captured"):
            wpt_report.parse_batch_output(output, self.names, 101)

    def test_chunks_names_without_reordering(self):
        with patch.object(wpt_report, "MAX_BATCH_NAMES", 2):
            self.assertEqual(
                list(wpt_report.batches(self.names)), [self.names[:2], self.names[2:]]
            )
        with patch.object(wpt_report, "MAX_BATCH_BYTES", 16):
            self.assertEqual(
                list(wpt_report.batches(self.names)), [[name] for name in self.names]
            )
        with patch.object(wpt_report, "MAX_BATCH_BYTES", 2):
            with self.assertRaises(ValueError):
                list(wpt_report.batches(self.names))

    def test_incomplete_run_does_not_overwrite_existing_report(self):
        with tempfile.TemporaryDirectory() as directory:
            latest = Path(directory) / "latest.json"
            latest.write_text("old report", encoding="utf-8")
            result = Mock(stdout="running 1 test\n", stderr="", returncode=0)
            with (
                patch.object(wpt_report, "list_wpt_tests", return_value=[self.names[0]]),
                patch.object(wpt_report, "LATEST_REPORT", latest),
                patch.object(wpt_report.subprocess, "run", return_value=result),
                contextlib.redirect_stdout(io.StringIO()),
                contextlib.redirect_stderr(io.StringIO()),
            ):
                self.assertEqual(wpt_report.main([]), 1)
            self.assertEqual(latest.read_text(encoding="utf-8"), "old report")

    def test_duplicate_discovered_names_are_rejected(self):
        result = Mock(
            stdout="wpt::css::pass: test\nwpt::css::pass: test\n", returncode=0
        )
        with (
            patch.object(wpt_report.subprocess, "run", return_value=result),
            contextlib.redirect_stderr(io.StringIO()),
        ):
            self.assertIsNone(wpt_report.list_wpt_tests())

    def test_metric_in_cargo_stderr_does_not_overwrite_existing_report(self):
        with tempfile.TemporaryDirectory() as directory:
            latest = Path(directory) / "latest.json"
            latest.write_text("old report", encoding="utf-8")
            result = Mock(
                stdout="",
                stderr='FREEDOM_WPT_METRIC {"suite":"url"}\n',
                returncode=0,
            )
            with (
                patch.object(wpt_report, "list_wpt_tests", return_value=[self.names[0]]),
                patch.object(wpt_report, "LATEST_REPORT", latest),
                patch.object(wpt_report.subprocess, "run", return_value=result),
                contextlib.redirect_stdout(io.StringIO()),
                contextlib.redirect_stderr(io.StringIO()),
            ):
                self.assertEqual(wpt_report.main([]), 1)
            self.assertEqual(latest.read_text(encoding="utf-8"), "old report")

    def test_dashboard_displays_ignored_counts_and_handles_old_reports(self):
        report = {
            "totals": {"tests": 2, "passed": 1, "failed": 0, "ignored": 1},
            "suites": {"css": {"tests": 2, "passed": 1, "failed": 0, "ignored": 1}},
            "exit_code": 0,
        }
        dashboard = wpt_dashboard.render_dashboard(report, [])
        self.assertIn("<span>Ignored</span><strong>1</strong>", dashboard)
        self.assertIn("<th>Ignored</th>", dashboard)
        self.assertIn(
            "<span>Ignored</span><strong>0</strong>",
            wpt_dashboard.render_dashboard({}, []),
        )


if __name__ == "__main__":
    unittest.main()
