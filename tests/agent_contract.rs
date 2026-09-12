use prooftrade::agent_contract::{
    AgentToolRegistry, PROFILE_NAME, TOOL_CONTRACT_VERSION, strict_output_schema, tool_specs,
};
use prooftrade::domain::TradingUniverse;
use prooftrade::operator::{OperatorReadModel, OperatorState};
use serde_json::json;

const PROMPT: &str = include_str!("../prompts/prooftrade-agent.md");
const CONTRACT: &str = include_str!("../docs/prooftrade-agent-contract.md");

#[test]
fn canonical_agent_artifacts_exist_and_name_the_real_profile() {
    assert_eq!(PROFILE_NAME, "prooftrade-agent");
    assert!(PROMPT.contains("prooftrade-agent"));
    assert!(CONTRACT.contains(TOOL_CONTRACT_VERSION));
}

#[test]
fn agent_tool_surface_has_no_direct_order_or_risk_bypass() {
    let specs = tool_specs();
    assert!(
        specs
            .iter()
            .any(|spec| spec.name == "submit_trade_decision")
    );
    assert!(specs.iter().all(|spec| !spec.mutates_exchange));
    assert!(
        specs
            .iter()
            .all(|spec| !spec.name.contains("raw_place_order"))
    );
    assert!(!PROMPT.contains("raw_place_order"));
}

#[test]
fn strict_output_schema_declares_every_required_property() {
    let schema = strict_output_schema();
    let required = schema["required"].as_array().unwrap();
    let properties = schema["properties"].as_object().unwrap();

    for field in required {
        let field = field.as_str().unwrap();
        assert!(
            properties.contains_key(field),
            "missing schema property: {field}"
        );
    }
}

#[test]
fn executable_registry_exposes_the_contract_tools() {
    let registry = AgentToolRegistry::new(
        TradingUniverse::initial(),
        OperatorState::new(OperatorReadModel::default()),
    );
    let tools = registry.list();
    assert_eq!(tools.len(), tool_specs().len());
    assert!(
        tools
            .iter()
            .any(|tool| tool["name"] == "submit_trade_decision")
    );
    assert!(registry.call("get_portfolio", json!({})).is_ok());
}

#[test]
fn submit_trade_decision_is_strict_and_never_an_exchange_mutation() {
    let registry = AgentToolRegistry::new(
        TradingUniverse::initial(),
        OperatorState::new(OperatorReadModel::default()),
    );
    let payload = json!({
        "schema_version": 1,
        "profile": "prooftrade-agent",
        "request_id": "request-1",
        "run_id": "run-1",
        "emitted_at": "2023-11-14T22:13:20Z",
        "decision_id": "00000000-0000-0000-0000-000000000101",
        "instrument": "BTC-USDT",
        "action": "BUY",
        "confidence": 0.8,
        "support_strength": 0.7,
        "counter_signal_strength": 0.2,
        "expected_horizon_secs": 300,
        "desired_exposure_pct": 0.1,
        "requested_notional": "10",
        "desired_reduction_pct": null,
        "urgency": "NORMAL",
        "max_acceptable_price": null,
        "why_trade": ["fixture evidence"],
        "why_not_trade": ["counter-signal remains bounded"],
        "reconsider_if": [],
        "invalidation": null,
        "evidence_refs": [],
        "created_at": "2023-11-14T22:13:20Z"
    });

    let response = registry
        .call("submit_trade_decision", payload)
        .expect("strict proposed intent");
    assert_eq!(response["status"], "PROPOSED_INTENT");
    assert_eq!(response["exchange_mutation"], false);
    assert_eq!(response["risk_evaluation"], "required_before_execution");

    let invalid = registry.call(
        "submit_trade_decision",
        json!({
            "request_id": "request-1",
            "payload": {"schema_version": 1, "profile": "prooftrade-agent"}
        }),
    );
    assert!(invalid.is_err());
}
