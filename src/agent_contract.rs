//! Reviewable contract metadata for the independent `prooftrade-agent` profile.

use serde_json::{Value, json};
use thiserror::Error;

pub const PROFILE_NAME: &str = "prooftrade-agent";
pub const TOOL_CONTRACT_VERSION: &str = "prooftrade-agent-tools/v1";
pub const PROMPT_PATH: &str = "prompts/prooftrade-agent.md";
pub const TOOL_CONTRACT_PATH: &str = "docs/prooftrade-agent-contract.md";

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct AgentToolSpec {
    pub name: &'static str,
    pub mutates_exchange: bool,
    pub purpose: &'static str,
}

pub const fn tool_specs() -> &'static [AgentToolSpec] {
    &[
        AgentToolSpec {
            name: "get_market_state",
            mutates_exchange: false,
            purpose: "read current OKX market state",
        },
        AgentToolSpec {
            name: "get_portfolio",
            mutates_exchange: false,
            purpose: "read runtime portfolio projection",
        },
        AgentToolSpec {
            name: "get_ranked_evidence",
            mutates_exchange: false,
            purpose: "read bounded ranked intelligence",
        },
        AgentToolSpec {
            name: "get_source_health",
            mutates_exchange: false,
            purpose: "read per-source health and degradation",
        },
        AgentToolSpec {
            name: "investigate_asset",
            mutates_exchange: false,
            purpose: "request bounded source research",
        },
        AgentToolSpec {
            name: "investigate_event",
            mutates_exchange: false,
            purpose: "inspect an event lineage",
        },
        AgentToolSpec {
            name: "submit_trade_decision",
            mutates_exchange: false,
            purpose: "submit strict proposed intent to runtime",
        },
        AgentToolSpec {
            name: "get_decision_outcome",
            mutates_exchange: false,
            purpose: "read risk and execution outcome",
        },
    ]
}

pub fn mcp_tool_definitions() -> Vec<Value> {
    tool_specs()
        .iter()
        .map(|spec| {
            json!({
                "name": spec.name,
                "description": spec.purpose,
                "inputSchema": {
                    "type": "object",
                    "additionalProperties": true
                }
            })
        })
        .collect()
}

pub fn strict_output_schema() -> Value {
    json!({
        "type": "object",
        "additionalProperties": false,
        "required": ["schema_version", "profile", "request_id", "run_id", "emitted_at", "decision_id", "instrument", "action", "confidence", "support_strength", "counter_signal_strength", "urgency", "why_trade", "why_not_trade", "evidence_refs", "created_at"],
        "properties": {
            "schema_version": {"const": 1},
            "profile": {"const": PROFILE_NAME},
            "request_id": {"type": "string", "minLength": 1},
            "run_id": {"type": "string", "minLength": 1},
            "emitted_at": {"type": "string"},
            "decision_id": {"type": "string", "format": "uuid"},
            "action": {"enum": ["BUY", "SELL", "HOLD"]},
            "instrument": {"type": "string"},
            "confidence": {"type": "number", "minimum": 0, "maximum": 1},
            "support_strength": {"type": "number", "minimum": 0, "maximum": 1},
            "counter_signal_strength": {"type": "number", "minimum": 0, "maximum": 1},
            "expected_horizon_secs": {"type": ["integer", "null"], "minimum": 1},
            "desired_exposure_pct": {"type": ["number", "null"], "minimum": 0, "maximum": 1},
            "requested_notional": {"type": ["string", "null"]},
            "desired_reduction_pct": {"type": ["number", "null"], "minimum": 0, "maximum": 1},
            "urgency": {"enum": ["LOW", "NORMAL", "HIGH"]},
            "max_acceptable_price": {"type": ["string", "null"]},
            "why_trade": {"type": "array", "items": {"type": "string"}},
            "why_not_trade": {"type": "array", "items": {"type": "string"}},
            "reconsider_if": {"type": "array", "items": {"type": "string"}},
            "invalidation": {"type": ["object", "null"]},
            "evidence_refs": {"type": "array"},
            "created_at": {"type": "string"}
        }
    })
}

#[derive(Debug, Error)]
pub enum AgentToolError {
    #[error("unknown prooftrade-agent tool: {0}")]
    UnknownTool(String),
    #[error("invalid tool arguments: {0}")]
    InvalidArguments(String),
    #[error("Hermes decision rejected: {0}")]
    DecisionRejected(#[from] crate::hermes::HermesError),
    #[error("tool response serialization failed: {0}")]
    Serialization(#[from] serde_json::Error),
}

#[derive(Clone)]
pub struct AgentToolRegistry {
    universe: crate::domain::TradingUniverse,
    state: crate::operator::OperatorState,
}

impl AgentToolRegistry {
    pub fn new(
        universe: crate::domain::TradingUniverse,
        state: crate::operator::OperatorState,
    ) -> Self {
        Self { universe, state }
    }

    pub fn list(&self) -> Vec<Value> {
        mcp_tool_definitions()
    }

    pub fn call(&self, name: &str, arguments: Value) -> Result<Value, AgentToolError> {
        let model = self.state.snapshot();
        match name {
            "get_market_state" => Ok(json!({
                "status": if model.market.is_empty() { "unavailable" } else { "available" },
                "environment": model.environment,
                "market": model.market,
                "reason": if model.market.is_empty() {
                    "market context is not present in the persisted operator projection"
                } else {
                    "persisted operator market snapshot"
                }
            })),
            "get_portfolio" => Ok(json!({
                "environment": model.environment,
                "portfolio": model.portfolio,
                "positions": model.positions,
                "safety_state": model.safety_state,
            })),
            "get_ranked_evidence" => {
                let limit = bounded_limit(&arguments)?;
                Ok(json!({
                    "signals": model.signals.iter().take(limit).collect::<Vec<_>>(),
                    "opportunities": model.opportunities.iter().take(limit).collect::<Vec<_>>(),
                    "bounded": true,
                }))
            }
            "get_source_health" => Ok(json!({
                "sources": model.sources,
                "errors": model.research.errors,
                "bounded": true,
            })),
            "investigate_asset" => {
                let instrument = required_string(&arguments, "instrument")?;
                let signals = model
                    .signals
                    .iter()
                    .filter(|signal| {
                        signal
                            .affected_assets
                            .iter()
                            .any(|asset| asset.instrument.to_string() == instrument)
                    })
                    .take(bounded_limit(&arguments)?)
                    .collect::<Vec<_>>();
                Ok(json!({
                    "instrument": instrument,
                    "status": if signals.is_empty() { "no_cached_evidence" } else { "cached_evidence" },
                    "signals": signals,
                    "network_access": false,
                }))
            }
            "investigate_event" => {
                let fingerprint = required_string(&arguments, "event_fingerprint")?;
                let signals = model
                    .signals
                    .iter()
                    .filter(|signal| signal.event_fingerprint.as_str() == fingerprint)
                    .take(bounded_limit(&arguments)?)
                    .collect::<Vec<_>>();
                Ok(json!({
                    "event_fingerprint": fingerprint,
                    "signals": signals,
                    "network_access": false,
                }))
            }
            "submit_trade_decision" => self.submit_trade_decision(arguments),
            "get_decision_outcome" => Ok(json!({
                "decision": model.latest_decision,
                "risk": model.latest_risk_decision,
                "executions": model.execution_records,
                "warning": model.execution_warning,
            })),
            other => Err(AgentToolError::UnknownTool(other.to_owned())),
        }
    }

    fn submit_trade_decision(&self, arguments: Value) -> Result<Value, AgentToolError> {
        let payload = arguments.get("payload").cloned().unwrap_or(arguments);
        let request_id = payload
            .get("request_id")
            .and_then(Value::as_str)
            .filter(|value| !value.trim().is_empty())
            .ok_or_else(|| AgentToolError::InvalidArguments("request_id is required".to_owned()))?;
        let request = crate::hermes::HermesRequest {
            request_id: request_id.to_owned(),
        };
        let validated = crate::hermes::decode_agent_decision_for_request(
            &serde_json::to_string(&payload)?,
            &self.universe,
            &request,
        )?;
        Ok(json!({
            "status": "PROPOSED_INTENT",
            "decision": validated.decision(),
            "provenance": {
                "schema_version": validated.provenance().schema_version,
                "profile": validated.provenance().profile,
                "request_id": validated.provenance().request_id,
                "run_id": validated.provenance().run_id,
                "emitted_at": validated.provenance().emitted_at,
            },
            "risk_evaluation": "required_before_execution",
            "exchange_mutation": false,
        }))
    }
}

fn bounded_limit(arguments: &Value) -> Result<usize, AgentToolError> {
    let value = arguments
        .get("limit")
        .and_then(Value::as_u64)
        .unwrap_or(20)
        .clamp(1, 100);
    usize::try_from(value)
        .map_err(|_| AgentToolError::InvalidArguments("limit is out of range".to_owned()))
}

fn required_string(arguments: &Value, field: &'static str) -> Result<String, AgentToolError> {
    arguments
        .get(field)
        .and_then(Value::as_str)
        .filter(|value| !value.trim().is_empty())
        .map(ToOwned::to_owned)
        .ok_or_else(|| AgentToolError::InvalidArguments(format!("{field} is required")))
}
