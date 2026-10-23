use std::{collections::HashMap, fs};

use crate::{
    javascript::{Value, parse_json},
    url::Url,
};

struct UrlCase {
    fixture_index: usize,
    input: String,
    base: Option<String>,
    expected_href: Option<String>,
    expected_failure: bool,
}

struct WptReport {
    total: usize,
    passed: usize,
    failures: Vec<String>,
}

fn run_urltestdata(source: &str) -> Result<WptReport, String> {
    let root = parse_json(source).map_err(|error| error.to_string())?;

    let records = root
        .as_array()
        .ok_or_else(|| "WPT URL fixture root must be an array".to_owned())?;

    let cases = collect_cases(records)?;

    let mut report = WptReport {
        total: cases.len(),
        passed: 0,
        failures: Vec::new(),
    };

    for case in cases {
        let actual = match case.base.as_deref() {
            Some(base) => Url::parse(base).and_then(|base_url| base_url.join(&case.input)),
            None => Url::parse(&case.input),
        };

        match (case.expected_failure, actual) {
            (true, Err(_)) => report.passed += 1,

            (true, Ok(url)) => {
                report.failures.push(format!(
                    "case {}: expected URL construction to fail, got {url}",
                    case.fixture_index
                ));
            }

            (false, Err(error)) => {
                report.failures.push(format!(
                    "case {}: expected {:?}, got error: {error}",
                    case.fixture_index, case.expected_href
                ));
            }

            (false, Ok(url)) => {
                let actual_href = url.to_string();
                let expected_href = case
                    .expected_href
                    .as_deref()
                    .expect("successful WPT case must have href");

                if actual_href == expected_href {
                    report.passed += 1;
                } else {
                    report.failures.push(format!(
                        "case {}: input {:?}, base {:?}; expected {expected_href:?}, got {actual_href:?}",
                        case.fixture_index, case.input, case.base
                    ));
                }
            }
        }
    }

    Ok(report)
}

fn collect_cases(records: &[Value]) -> Result<Vec<UrlCase>, String> {
    let mut cases = Vec::new();

    for (fixture_index, record) in records.iter().enumerate() {
        let Some(object) = record.as_object() else {
            continue;
        };

        let Some(input) = object_string(object, "input") else {
            continue;
        };

        let expected_failure = object
            .get("failure")
            .and_then(Value::as_bool)
            .unwrap_or(false);

        let expected_href = object_string(object, "href").map(str::to_owned);

        if !expected_failure && expected_href.is_none() {
            return Err(format!(
                "case {fixture_index}: successful URL case has no href"
            ));
        }

        cases.push(UrlCase {
            fixture_index,
            input: input.to_owned(),
            base: object_string(object, "base").map(str::to_owned),
            expected_href,
            expected_failure,
        });
    }

    Ok(cases)
}

fn object_string<'a>(object: &'a HashMap<String, Value>, name: &str) -> Option<&'a str> {
    object.get(name).and_then(Value::as_string)
}

fn fixture_path() -> String {
    format!(
        "{}/testdata/wpt/url/urltestdata.json",
        env!("CARGO_MANIFEST_DIR")
    )
}

fn display_failures(report: &WptReport) -> String {
    report
        .failures
        .iter()
        .take(20)
        .cloned()
        .collect::<Vec<_>>()
        .join("\n")
}

#[cfg(test)]
mod tests {
    use super::{display_failures, fixture_path, fs, run_urltestdata};

    #[test]
    fn runs_a_small_wpt_style_url_fixture() {
        let report = run_urltestdata(
            r##"
            [
                {
                    "input": "#",
                    "base": "http://example.org/foo/bar",
                    "href": "http://example.org/foo/bar#"
                },
                {
                    "input": "http://example.org:bad/",
                    "failure": true
                }
            ]
            "##,
        )
        .unwrap();

        assert_eq!(report.total, 2);
        assert_eq!(report.passed, 2);
        assert!(report.failures.is_empty());
    }

    #[test]
    fn official_wpt_urltestdata() {
        let source = fs::read_to_string(fixture_path())
            .expect("WPT fixture is missing; see the project instructions");

        let report = run_urltestdata(&source).unwrap();

        eprintln!(
            "WPT URL result: {}/{} passed, {} failed",
            report.passed,
            report.total,
            report.failures.len()
        );

        println!(
            "FREEDOM_WPT_METRIC {{\"suite\":\"url\",\"kind\":\"cases\",\"passed\":{},\"total\":{},\"failed\":{}}}",
            report.passed,
            report.total,
            report.failures.len()
        );

        assert!(
            report.failures.is_empty(),
            "first failures:\n{}",
            display_failures(&report)
        );
    }
}
