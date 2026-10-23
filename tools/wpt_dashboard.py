#!/usr/bin/env python3

import json
from html import escape
from pathlib import Path

ROOT = Path(__file__).resolve().parents[1]
REPORT_DIRECTORY = ROOT / "artifacts" / "wpt"
LATEST_REPORT = REPORT_DIRECTORY / "latest.json"
HISTORY_REPORT = REPORT_DIRECTORY / "history.json"
DASHBOARD = REPORT_DIRECTORY / "index.html"


def load_json(path, fallback):
    if not path.exists():
        return fallback

    try:
        value = json.loads(path.read_text(encoding="utf-8"))
    except json.JSONDecodeError:
        return fallback

    return value


def count(mapping, name):
    value = mapping.get(name, 0)

    return value if isinstance(value, int) else 0


def pass_rate(report):
    totals = report.get("totals", {})
    tests = count(totals, "tests")

    if tests == 0:
        return 0.0

    return count(totals, "passed") * 100.0 / tests


def measurement_cards(report):
    measurements = report.get("measurements", {})

    if not isinstance(measurements, dict):
        return ""

    cards = []

    for name in sorted(measurements):
        measurement = measurements[name]

        if not isinstance(measurement, dict):
            continue

        passed = count(measurement, "passed")
        total = count(measurement, "total")
        failed = count(measurement, "failed")
        color = "pass" if failed == 0 else "fail"
        label = escape(name.replace("_", " ").upper())

        cards.append(
            '<div class="card">'
            f"<span>{label}</span>"
            f'<strong class="{color}">{passed}/{total}</strong>'
            "</div>"
        )

    return "\n".join(cards)


def history_chart(history):
    reports = [
        report
        for report in history
        if isinstance(report, dict) and isinstance(report.get("totals"), dict)
    ][-60:]

    if not reports:
        return "<p>No historical runs are available yet.</p>"

    width = 720
    height = 180
    padding = 18
    plot_width = width - padding * 2
    plot_height = height - padding * 2

    points = []
    circles = []

    for index, report in enumerate(reports):
        if len(reports) == 1:
            x = width / 2
        else:
            x = padding + plot_width * index / (len(reports) - 1)

        y = padding + plot_height * (1.0 - pass_rate(report) / 100.0)
        points.append(f"{x:.2f},{y:.2f}")

        timestamp = escape(str(report.get("generated_at", "unknown time")))
        rate = pass_rate(report)

        circles.append(
            f'<circle cx="{x:.2f}" cy="{y:.2f}" r="3">'
            f"<title>{timestamp}: {rate:.1f}% passed</title>"
            "</circle>"
        )

    first_time = escape(str(reports[0].get("generated_at", "unknown time")))
    last_time = escape(str(reports[-1].get("generated_at", "unknown time")))

    return f"""
        <figure class="history">
          <svg viewBox="0 0 {width} {height}" role="img"
               aria-label="WPT pass-rate history">
            <line x1="{padding}" y1="{padding}" x2="{width - padding}" y2="{padding}"/>
            <line x1="{padding}" y1="{height / 2}" x2="{width - padding}" y2="{height / 2}"/>
            <line x1="{padding}" y1="{height - padding}" x2="{width - padding}" y2="{height - padding}"/>
            <polyline points="{' '.join(points)}"/>
            {''.join(circles)}
          </svg>
          <figcaption>
            Pass-rate history: {first_time} → {last_time}
          </figcaption>
        </figure>
    """


def suite_rows(report):
    suites = report.get("suites", {})

    if not isinstance(suites, dict) or not suites:
        return '<tr><td colspan="5">No suite data available.</td></tr>'

    rows = []

    for name in sorted(suites):
        suite = suites[name]

        if not isinstance(suite, dict):
            continue

        tests = count(suite, "tests")
        passed = count(suite, "passed")
        failed = count(suite, "failed")
        rate = passed * 100.0 / tests if tests else 0.0

        rows.append(
            "<tr>"
            f"<th scope=\"row\">{escape(name.upper())}</th>"
            f"<td>{tests}</td>"
            f"<td class=\"pass\">{passed}</td>"
            f"<td class=\"fail\">{failed}</td>"
            f"<td>{rate:.1f}%</td>"
            "</tr>"
        )

    return "\n".join(rows)


def failure_list(report):
    failures = report.get("failures", [])

    if not isinstance(failures, list) or not failures:
        return '<p class="success">No failed WPT test units.</p>'

    items = "\n".join(f"<li><code>{escape(str(name))}</code></li>" for name in failures)

    return f"<ul>{items}</ul>"


def render_dashboard(report, history):
    totals = report.get("totals", {})
    passed = count(totals, "passed")
    failed = count(totals, "failed")
    tests = count(totals, "tests")
    rate = pass_rate(report)
    revision = escape(str(report.get("git_revision", "unknown")))
    timestamp = escape(str(report.get("generated_at", "unknown time")))
    status = "passing" if report.get("exit_code") == 0 else "failing"

    return f"""<!doctype html>
<html lang="en">
<head>
  <meta charset="utf-8">
  <meta name="viewport" content="width=device-width, initial-scale=1">
  <title>FreeDOM WPT dashboard</title>
  <style>
    :root {{
        color-scheme: dark;
        font-family: system-ui, sans-serif;
        background: #10151d;
        color: #e9eef5;
    }}

    body {{
        margin: 0;
        padding: 32px;
    }}

    main {{
        max-width: 980px;
        margin: 0 auto;
    }}

    h1, h2 {{
        margin: 0 0 12px;
    }}

    .subtitle {{
        color: #aeb9c8;
        margin: 0 0 28px;
    }}

    .status {{
        color: #5ee58b;
        font-weight: 700;
    }}

    .status.failing {{
        color: #ff7474;
    }}

    .cards {{
        display: grid;
        grid-template-columns: repeat(auto-fit, minmax(160px, 1fr));
        gap: 12px;
        margin: 20px 0 32px;
    }}

    .card, section {{
        border: 1px solid #2b3645;
        border-radius: 10px;
        background: #18212c;
        padding: 18px;
    }}

    .card strong {{
        display: block;
        font-size: 2rem;
    }}

    section {{
        margin-top: 20px;
    }}

    table {{
        border-collapse: collapse;
        width: 100%;
    }}

    th, td {{
        padding: 10px;
        border-bottom: 1px solid #2b3645;
        text-align: left;
    }}

    .pass, .success {{
        color: #5ee58b;
    }}

    .fail {{
        color: #ff7474;
    }}

    .history svg {{
        display: block;
        width: 100%;
        max-width: 720px;
        background: #10151d;
        border-radius: 8px;
    }}

    .history line {{
        stroke: #354256;
        stroke-width: 1;
    }}

    .history polyline {{
        fill: none;
        stroke: #59a7ff;
        stroke-width: 3;
    }}

    .history circle {{
        fill: #e9eef5;
    }}

    figcaption {{
        color: #aeb9c8;
        font-size: 0.85rem;
        margin-top: 8px;
    }}

    code {{
        color: #a8d4ff;
    }}
  </style>
</head>
<body>
  <main>
    <header>
      <h1>FreeDOM WPT dashboard</h1>
      <p class="subtitle">
        Latest run: <span class="status {status}">{status}</span>
        * {timestamp}
        * revision <code>{revision}</code>
      </p>
    </header>

    <div class="cards">
      <div class="card"><span>WPT test units</span><strong>{tests}</strong></div>
      {measurement_cards(report)}
      <div class="card"><span>Passed</span><strong class="pass">{passed}</strong></div>
      <div class="card"><span>Failed</span><strong class="fail">{failed}</strong></div>
      <div class="card"><span>Pass rate</span><strong>{rate:.1f}%</strong></div>
    </div>

    <section>
      <h2>Suite results</h2>
      <table>
        <thead>
          <tr>
            <th>Suite</th>
            <th>Tests</th>
            <th>Passed</th>
            <th>Failed</th>
            <th>Pass rate</th>
          </tr>
        </thead>
        <tbody>
          {suite_rows(report)}
        </tbody>
      </table>
    </section>

    <section>
      <h2>History</h2>
      {history_chart(history)}
    </section>

    <section>
      <h2>Failures</h2>
      {failure_list(report)}
    </section>
  </main>
</body>
</html>
"""


def main():
    report = load_json(LATEST_REPORT, {})
    history = load_json(HISTORY_REPORT, {})

    if not isinstance(report, dict):
        raise SystemExit("Latest WPT report is invalid.")

    if not isinstance(history, list):
        history = []

    DASHBOARD.parent.mkdir(parents=True, exist_ok=True)
    DASHBOARD.write_text(render_dashboard(report, history), encoding="utf-8")

    print(f"Wrote {DASHBOARD.relative_to(ROOT)}")


if __name__ == "__main__":
    main()