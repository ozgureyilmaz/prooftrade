use prooftrade::domain::TradeAction;
use prooftrade::domain::TradingUniverse;
use prooftrade::hermes::{
    HermesClient, HermesError, HermesRequest, decode_agent_decision,
    decode_agent_decision_for_request,
};
use rust_decimal::Decimal;
use serde_json::{Value, json};

fn universe() -> TradingUniverse {
    TradingUniverse::initial()
}

fn valid_buy_payload() -> Value {
    json!({
        "schema_version": 1,
        "profile": "prooftrade-agent",
        "request_id": "request-1",
        "run_id": "run-1",
        "emitted_at": "2026-09-12T08:00:00Z",
        "decision_id": "11111111-1111-4111-8111-111111111111",
        "instrument": "SOL-USDT",
        "action": "BUY",
        "confidence": 0.84,
        "support_strength": 0.88,
        "counter_signal_strength": 0.22,
        "expected_horizon_secs": 900,
        "desired_exposure_pct": 0.18,
        "requested_notional": "150.00",
        "urgency": "HIGH",
        "max_acceptable_price": "142.35",
        "why_trade": ["fresh catalyst", "OKX confirmation"],
        "why_not_trade": ["Reddit confirmation is weak"],
        "reconsider_if": ["volume confirms breakout"],
        "invalidation": {"description": "catalyst loses confirmation"},
        "evidence_refs": [{
            "evidence_id": "22222222-2222-4222-8222-222222222222",
            "source": "marx_finance",
            "source_event_id": "marx-event-1",
            "lineage_id": "33333333-3333-4333-8333-333333333333",
            "instrument": "SOL-USDT",
            "observed_at": "2026-09-12T07:59:00Z",
            "direction": "supporting",
            "strength": 0.80,
            "freshness": 0.90,
            "reliability": 0.85,
            "novelty": 0.70,
            "relevance": 0.95,
            "source_reference": "marx://event-1"
        }],
        "created_at": "2026-09-12T08:00:00Z"
    })
}

fn encode(payload: Value) -> String {
    serde_json::to_string(&payload).unwrap()
}

struct FakeHermesClient {
    payload: String,
}

impl HermesClient for FakeHermesClient {
    type Error = ();

    fn request_decision(&self, _request: &HermesRequest) -> Result<String, Self::Error> {
        Ok(self.payload.clone())
    }
}

#[test]
fn malformed_hermes_json_is_rejected_as_invalid_agent_decision() {
    let error = decode_agent_decision("{not-json", &universe()).unwrap_err();

    assert!(matches!(error, HermesError::MalformedPayload(_)));
    assert!(error.means_no_trade());
}

#[test]
fn unknown_hermes_fields_are_rejected() {
    let error = decode_agent_decision(
        r#"{
            "schema_version": 1,
            "profile": "prooftrade-agent",
            "request_id": "request-1",
            "run_id": "run-1",
            "emitted_at": "2026-09-12T08:00:00Z",
            "decision_id": "11111111-1111-4111-8111-111111111111",
            "instrument": "BTC-USDT",
            "action": "BUY",
            "confidence": 0.8,
            "support_strength": 0.8,
            "counter_signal_strength": 0.1,
            "urgency": "HIGH",
            "created_at": "2026-09-12T08:00:00Z",
            "unexpected": true
        }"#,
        &universe(),
    )
    .unwrap_err();

    assert!(matches!(error, HermesError::MalformedPayload(_)));
    assert!(error.means_no_trade());
}

#[test]
fn valid_buy_is_converted_without_losing_provenance_or_evidence_lineage() {
    let validated = decode_agent_decision(&encode(valid_buy_payload()), &universe()).unwrap();

    assert_eq!(validated.decision().action, TradeAction::Buy);
    assert_eq!(validated.decision().instrument.symbol(), "SOL-USDT");
    assert_eq!(validated.decision().confidence, Decimal::new(84, 2));
    assert_eq!(validated.decision().support_strength, Decimal::new(88, 2));
    assert_eq!(
        validated.decision().counter_signal_strength,
        Decimal::new(22, 2)
    );
    assert_eq!(
        validated.decision().requested_notional,
        Some(Decimal::new(15_000, 2))
    );
    assert_eq!(validated.decision().evidence_refs.len(), 1);
    assert_eq!(
        validated.decision().evidence_refs[0]
            .source_event_id
            .as_str(),
        "marx-event-1"
    );
    assert_eq!(validated.provenance().request_id, "request-1");
    assert_eq!(validated.provenance().run_id, "run-1");
}

#[test]
fn valid_hold_is_a_first_class_no_exposure_change_decision() {
    let mut payload = valid_buy_payload();
    let object = payload.as_object_mut().unwrap();
    object.insert("action".to_owned(), json!("HOLD"));
    object.insert(
        "why_not_trade".to_owned(),
        json!(["catalyst is already priced"]),
    );
    object.remove("desired_exposure_pct");
    object.remove("requested_notional");
    object.remove("desired_reduction_pct");
    object.remove("max_acceptable_price");

    let validated = decode_agent_decision(&encode(payload), &universe()).unwrap();

    assert_eq!(validated.decision().action, TradeAction::Hold);
    assert!(validated.decision().requested_notional.is_none());
    assert!(validated.decision().desired_exposure_pct.is_none());
    assert!(validated.decision().desired_reduction_pct.is_none());
}

#[test]
fn valid_partial_sell_preserves_reduction_intent_without_inventory_logic() {
    let mut payload = valid_buy_payload();
    let object = payload.as_object_mut().unwrap();
    object.insert("action".to_owned(), json!("SELL"));
    object.insert("desired_reduction_pct".to_owned(), json!(0.40));
    object.remove("desired_exposure_pct");
    object.remove("requested_notional");
    object.remove("max_acceptable_price");

    let validated = decode_agent_decision(&encode(payload), &universe()).unwrap();

    assert_eq!(validated.decision().action, TradeAction::Sell);
    assert_eq!(
        validated.decision().desired_reduction_pct,
        Some(Decimal::new(40, 2))
    );
}

#[test]
fn unsupported_schema_version_is_rejected_before_domain_conversion() {
    let mut payload = valid_buy_payload();
    payload["schema_version"] = json!(2);

    let error = decode_agent_decision(&encode(payload), &universe()).unwrap_err();

    assert!(matches!(error, HermesError::UnsupportedSchemaVersion(2)));
    assert!(error.means_no_trade());
}

#[test]
fn unknown_instrument_is_rejected() {
    let mut payload = valid_buy_payload();
    payload["instrument"] = json!("DOGE-USDT");

    let error = decode_agent_decision(&encode(payload), &universe()).unwrap_err();

    assert!(matches!(error, HermesError::UnknownInstrument(_)));
}

#[test]
fn invalid_confidence_is_rejected() {
    let mut payload = valid_buy_payload();
    payload["confidence"] = json!(1.01);

    let error = decode_agent_decision(&encode(payload), &universe()).unwrap_err();

    assert!(matches!(error, HermesError::DomainValidation(_)));
}

#[test]
fn negative_requested_notional_is_rejected() {
    let mut payload = valid_buy_payload();
    payload["requested_notional"] = json!("-1.00");

    let error = decode_agent_decision(&encode(payload), &universe()).unwrap_err();

    assert!(matches!(error, HermesError::DomainValidation(_)));
}

#[test]
fn missing_action_is_rejected_as_malformed_payload() {
    let mut payload = valid_buy_payload();
    payload.as_object_mut().unwrap().remove("action");

    let error = decode_agent_decision(&encode(payload), &universe()).unwrap_err();

    assert!(matches!(error, HermesError::MalformedPayload(_)));
}

#[test]
fn unknown_action_is_rejected_as_malformed_payload() {
    let mut payload = valid_buy_payload();
    payload["action"] = json!("SHORT");

    let error = decode_agent_decision(&encode(payload), &universe()).unwrap_err();

    assert!(matches!(error, HermesError::MalformedPayload(_)));
}

#[test]
fn unexpected_hermes_profile_is_rejected() {
    let mut payload = valid_buy_payload();
    payload["profile"] = json!("other-agent");

    let error = decode_agent_decision(&encode(payload), &universe()).unwrap_err();

    assert!(matches!(error, HermesError::UnexpectedProfile(_)));
}

#[test]
fn empty_request_id_is_rejected() {
    let mut payload = valid_buy_payload();
    payload["request_id"] = json!("  ");

    let error = decode_agent_decision(&encode(payload), &universe()).unwrap_err();

    assert!(matches!(error, HermesError::MissingField("request_id")));
}

#[test]
fn exposure_above_one_is_rejected() {
    let mut payload = valid_buy_payload();
    payload["desired_exposure_pct"] = json!(1.01);

    let error = decode_agent_decision(&encode(payload), &universe()).unwrap_err();

    assert!(matches!(error, HermesError::DomainValidation(_)));
}

#[test]
fn hold_with_sizing_is_rejected() {
    let mut payload = valid_buy_payload();
    let object = payload.as_object_mut().unwrap();
    object.insert("action".to_owned(), json!("HOLD"));
    object.insert(
        "why_not_trade".to_owned(),
        json!(["evidence is insufficient"]),
    );
    object.remove("desired_exposure_pct");
    object.remove("requested_notional");
    object.remove("desired_reduction_pct");

    let error = decode_agent_decision(&encode(payload), &universe()).unwrap_err();

    assert!(matches!(error, HermesError::DomainValidation(_)));
}

#[test]
fn hold_without_a_reason_is_rejected() {
    let mut payload = valid_buy_payload();
    let object = payload.as_object_mut().unwrap();
    object.insert("action".to_owned(), json!("HOLD"));
    object.insert("why_not_trade".to_owned(), json!([]));
    object.remove("desired_exposure_pct");
    object.remove("requested_notional");
    object.remove("desired_reduction_pct");
    object.remove("max_acceptable_price");

    let error = decode_agent_decision(&encode(payload), &universe()).unwrap_err();

    assert!(matches!(error, HermesError::DomainValidation(_)));
}

#[test]
fn reduction_above_one_is_rejected() {
    let mut payload = valid_buy_payload();
    let object = payload.as_object_mut().unwrap();
    object.insert("action".to_owned(), json!("SELL"));
    object.insert("desired_reduction_pct".to_owned(), json!(1.01));
    object.remove("desired_exposure_pct");
    object.remove("requested_notional");
    object.remove("max_acceptable_price");

    let error = decode_agent_decision(&encode(payload), &universe()).unwrap_err();

    assert!(matches!(error, HermesError::DomainValidation(_)));
}

#[test]
fn evidence_with_another_instrument_is_rejected() {
    let mut payload = valid_buy_payload();
    payload["evidence_refs"][0]["instrument"] = json!("BTC-USDT");

    let error = decode_agent_decision(&encode(payload), &universe()).unwrap_err();

    assert!(matches!(error, HermesError::DomainValidation(_)));
}

#[test]
fn invalid_evidence_strength_is_rejected() {
    let mut payload = valid_buy_payload();
    payload["evidence_refs"][0]["strength"] = json!(1.01);

    let error = decode_agent_decision(&encode(payload), &universe()).unwrap_err();

    assert!(matches!(error, HermesError::DomainValidation(_)));
}

#[test]
fn invalid_nested_evidence_field_is_rejected() {
    let mut payload = valid_buy_payload();
    payload["evidence_refs"][0]["extra"] = json!(true);

    let error = decode_agent_decision(&encode(payload), &universe()).unwrap_err();

    assert!(matches!(error, HermesError::MalformedPayload(_)));
}

#[test]
fn free_form_prose_alone_cannot_become_a_trade_decision() {
    let error = decode_agent_decision(r#"{"reasoning":"SOL looks good, buy some."}"#, &universe())
        .unwrap_err();

    assert!(matches!(error, HermesError::MalformedPayload(_)));
    assert!(error.means_no_trade());
}

#[test]
fn singular_horizon_field_alias_is_preserved_as_canonical_seconds() {
    let mut payload = valid_buy_payload();
    let object = payload.as_object_mut().unwrap();
    let horizon = object.remove("expected_horizon_secs").unwrap();
    object.insert("expected_horizon_sec".to_owned(), horizon);

    let validated = decode_agent_decision(&encode(payload), &universe()).unwrap();

    assert_eq!(validated.decision().expected_horizon_secs, Some(900));
}

#[test]
fn transport_seam_returns_payload_for_the_explicit_decoder() {
    let client = FakeHermesClient {
        payload: encode(valid_buy_payload()),
    };
    let payload = client
        .request_decision(&HermesRequest {
            request_id: "request-1".to_owned(),
        })
        .unwrap();

    let validated = decode_agent_decision(&payload, &universe()).unwrap();

    assert_eq!(validated.decision().action, TradeAction::Buy);
}

#[test]
fn decision_request_provenance_mismatch_is_rejected() {
    let payload = encode(valid_buy_payload());
    let request = HermesRequest {
        request_id: "different-request".to_owned(),
    };

    let error = decode_agent_decision_for_request(&payload, &universe(), &request).unwrap_err();

    assert!(matches!(
        error,
        HermesError::InvalidField {
            field: "request_id",
            ..
        }
    ));
    assert!(error.means_no_trade());
}

#[test]
fn zero_horizon_is_rejected() {
    let mut payload = valid_buy_payload();
    payload["expected_horizon_secs"] = json!(0);

    let error = decode_agent_decision(&encode(payload), &universe()).unwrap_err();

    assert!(matches!(error, HermesError::DomainValidation(_)));
}

#[test]
fn non_positive_maximum_price_is_rejected() {
    let mut payload = valid_buy_payload();
    payload["max_acceptable_price"] = json!(0);

    let error = decode_agent_decision(&encode(payload), &universe()).unwrap_err();

    assert!(matches!(error, HermesError::DomainValidation(_)));
}

#[test]
fn non_finite_numeric_representation_is_rejected() {
    let mut payload = valid_buy_payload();
    payload["confidence"] = json!("NaN");

    let error = decode_agent_decision(&encode(payload), &universe()).unwrap_err();

    assert!(matches!(error, HermesError::MalformedPayload(_)));
}

#[test]
fn nil_decision_id_is_rejected() {
    let mut payload = valid_buy_payload();
    payload["decision_id"] = json!("00000000-0000-0000-0000-000000000000");

    let error = decode_agent_decision(&encode(payload), &universe()).unwrap_err();

    assert!(matches!(
        error,
        HermesError::InvalidField {
            field: "decision_id",
            ..
        }
    ));
}

#[test]
fn sell_cannot_carry_buy_sizing_fields() {
    let mut payload = valid_buy_payload();
    let object = payload.as_object_mut().unwrap();
    object.insert("action".to_owned(), json!("SELL"));
    object.insert("desired_reduction_pct".to_owned(), json!(0.40));
    object.remove("max_acceptable_price");

    let error = decode_agent_decision(&encode(payload), &universe()).unwrap_err();

    assert!(matches!(error, HermesError::DomainValidation(_)));
}

#[test]
fn rejected_payload_exposes_invalid_agent_decision_and_no_trade_codes() {
    let error = decode_agent_decision("{not-json", &universe()).unwrap_err();

    assert_eq!(error.code(), "INVALID_AGENT_DECISION");
    assert_eq!(error.outcome(), "NO_TRADE");
}
