use serde_json::Value;
use stasis_runner::live::{LiveRequest, LiveResponse, LIVE_SCHEMA_VERSION};

const REQUESTS: &str =
    include_str!("../../../tests/characterization/live_protocol/v1/requests.jsonl");
const RESPONSES: &str =
    include_str!("../../../tests/characterization/live_protocol/v1/responses.jsonl");
const MALFORMED: &str =
    include_str!("../../../tests/characterization/live_protocol/v1/malformed.jsonl");

fn records(source: &str) -> impl Iterator<Item = Value> + '_ {
    source.lines().enumerate().map(|(line, value)| {
        serde_json::from_str(value)
            .unwrap_or_else(|error| panic!("fixture line {} is invalid JSON: {error}", line + 1))
    })
}

fn payload(record: &Value) -> &Value {
    record
        .get("payload")
        .unwrap_or_else(|| panic!("{} has no payload", record["case"]))
}

fn expected(record: &Value) -> &str {
    record
        .get("expect")
        .and_then(Value::as_str)
        .unwrap_or_else(|| panic!("{} has no expectation", record["case"]))
}

fn response_shape_is_valid(response: &LiveResponse) -> bool {
    response.schema_version == LIVE_SCHEMA_VERSION
        && response.request_id > 0
        && !response.kind.is_empty()
}

#[test]
fn shared_request_fixtures_split_json_shape_from_rust_semantics() {
    let mut valid = 0;
    let mut semantic_invalid = 0;
    for record in records(REQUESTS) {
        let parsed = serde_json::from_value::<LiveRequest>(payload(&record).clone());
        match expected(&record) {
            "valid" => {
                let request =
                    parsed.unwrap_or_else(|error| panic!("{} shape: {error}", record["case"]));
                request
                    .validate()
                    .unwrap_or_else(|error| panic!("{} semantics: {error}", record["case"]));
                valid += 1;
            }
            "semantic_invalid" => {
                let request =
                    parsed.unwrap_or_else(|error| panic!("{} shape: {error}", record["case"]));
                assert!(
                    request.validate().is_err(),
                    "{} must fail Rust semantic validation",
                    record["case"]
                );
                semantic_invalid += 1;
            }
            other => panic!("unexpected request expectation {other:?}"),
        }
    }
    assert_eq!(valid, 5);
    assert_eq!(semantic_invalid, 2);
}

#[test]
fn shared_response_fixtures_cover_success_failure_truncation_and_identity() {
    let mut valid = 0;
    for record in records(RESPONSES) {
        assert_eq!(expected(&record), "valid", "response fixture expectation");
        let response = serde_json::from_value::<LiveResponse>(payload(&record).clone())
            .unwrap_or_else(|error| panic!("{} shape: {error}", record["case"]));
        assert!(
            response_shape_is_valid(&response),
            "{} shape",
            record["case"]
        );
        valid += 1;
    }
    assert_eq!(valid, 4);
}

#[test]
fn malformed_shared_fixtures_are_rejected_by_the_declared_protocol_parser() {
    for record in records(MALFORMED) {
        let protocol = record
            .get("protocol")
            .and_then(Value::as_str)
            .unwrap_or_else(|| panic!("{} has no protocol", record["case"]));
        let shape_valid = match protocol {
            "request" => serde_json::from_value::<LiveRequest>(payload(&record).clone()).is_ok_and(
                |request| request.schema_version == LIVE_SCHEMA_VERSION && request.request_id > 0,
            ),
            "response" => serde_json::from_value::<LiveResponse>(payload(&record).clone())
                .is_ok_and(|response| response_shape_is_valid(&response)),
            other => panic!("{} has unknown protocol {other}", record["case"]),
        };
        assert!(!shape_valid, "{} must be malformed", record["case"]);
        assert_eq!(expected(&record), "shape_invalid");
    }
}
