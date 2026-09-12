use std::collections::VecDeque;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use prooftrade::atk::{
    AccountSnapshot, AtkClient, AtkConfig, AtkError, AtkHealth, AtkSite, CancelOrderRequest,
    CancellationOutcome, McpError, McpTransport, OrderQuery, OrderSide, OrderState, OrderStatus,
    PlacementOutcome, RequiredCapability, SizeUnit, SpotOrderRequest, TradingMode,
};
use prooftrade::domain::Instrument;
use rust_decimal::Decimal;
use serde_json::{Value, json};

#[derive(Clone, Debug)]
struct RecordingTransport {
    responses: Arc<Mutex<VecDeque<Result<Value, McpError>>>>,
    calls: Arc<Mutex<Vec<(String, Value)>>>,
    notifications: Arc<Mutex<Vec<(String, Value)>>>,
    alive: Arc<Mutex<bool>>,
}

impl Default for RecordingTransport {
    fn default() -> Self {
        Self {
            responses: Arc::new(Mutex::new(VecDeque::new())),
            calls: Arc::new(Mutex::new(Vec::new())),
            notifications: Arc::new(Mutex::new(Vec::new())),
            alive: Arc::new(Mutex::new(true)),
        }
    }
}

impl RecordingTransport {
    fn push(&self, response: Result<Value, McpError>) {
        self.responses.lock().unwrap().push_back(response);
    }

    fn calls(&self) -> Vec<(String, Value)> {
        self.calls.lock().unwrap().clone()
    }

    fn mark_dead(&self) {
        *self.alive.lock().unwrap() = false;
    }
}

impl McpTransport for RecordingTransport {
    fn request(&self, method: &str, params: Value, _timeout: Duration) -> Result<Value, McpError> {
        self.calls.lock().unwrap().push((method.to_owned(), params));
        if !*self.alive.lock().unwrap() {
            return Err(McpError::ProcessExited);
        }
        self.responses
            .lock()
            .unwrap()
            .pop_front()
            .unwrap_or(Err(McpError::ProcessExited))
    }

    fn notify(&self, method: &str, params: Value) -> Result<(), McpError> {
        self.notifications
            .lock()
            .unwrap()
            .push((method.to_owned(), params));
        Ok(())
    }

    fn is_alive(&self) -> bool {
        *self.alive.lock().unwrap()
    }

    fn shutdown(&self) -> Result<(), McpError> {
        self.mark_dead();
        Ok(())
    }
}

fn response(id: u64, result: Value) -> Value {
    json!({"jsonrpc":"2.0", "id": id, "result": result})
}

fn initialize_response() -> Value {
    response(
        1,
        json!({
            "protocolVersion": "2025-06-18",
            "capabilities": {"tools": {}},
            "serverInfo": {"name": "okx-trade-mcp", "version": "1.4.6"}
        }),
    )
}

fn tool(name: &str, required: &[&str]) -> Value {
    let mut properties = serde_json::Map::new();
    for field in required {
        properties.insert((*field).to_owned(), json!({"type": "string"}));
    }
    json!({
        "name": name,
        "description": name,
        "inputSchema": {
            "type": "object",
            "properties": properties,
            "required": required,
        }
    })
}

fn tools_response(include_cancel: bool) -> Value {
    let mut tools = vec![
        tool("system_get_capabilities", &[]),
        tool("market_get_ticker", &["instId"]),
        tool("account_get_balance", &[]),
        tool(
            "spot_place_order",
            &["instId", "tdMode", "side", "ordType", "sz"],
        ),
        tool("spot_get_order", &["instId"]),
    ];
    if include_cancel {
        tools.push(tool("spot_cancel_order", &["instId"]));
    }
    response(2, json!({"tools": tools}))
}

fn capability_response(demo: bool, read_only: bool, site: Option<&str>) -> Value {
    let mut data = json!({
        "readOnly": read_only,
        "hasAuth": true,
        "demo": demo,
        "moduleAvailability": {
            "market": {"status":"enabled"},
            "account": {"status":"enabled"},
            "spot": {"status":"enabled"}
        }
    });
    if let Some(site) = site {
        data["site"] = json!(site);
    }
    tool_result("system_get_capabilities", data, 3)
}

fn nested_capability_response() -> Value {
    tool_result(
        "system_get_capabilities",
        json!({
            "server": {"name":"okx-trade-mcp", "version":"1.4.6"},
            "capabilities": {
                "readOnly": false,
                "hasAuth": true,
                "demo": true,
                "moduleAvailability": {
                    "market": {"status":"enabled"},
                    "account": {"status":"enabled"},
                    "spot": {"status":"enabled"}
                }
            }
        }),
        3,
    )
}

fn tool_result(tool_name: &str, data: Value, id: u64) -> Value {
    let payload = json!({
        "tool": tool_name,
        "ok": true,
        "data": data,
        "timestamp": "2026-09-12T08:00:00Z"
    });
    response(
        id,
        json!({
            "content": [{"type":"text", "text": serde_json::to_string(&payload).unwrap()}],
            "structuredContent": payload
        }),
    )
}

fn ready_transport() -> RecordingTransport {
    let transport = RecordingTransport::default();
    transport.push(Ok(initialize_response()));
    transport.push(Ok(tools_response(true)));
    transport.push(Ok(capability_response(true, false, Some("tr"))));
    transport
}

fn config(read_only: bool) -> AtkConfig {
    AtkConfig::new("okx-trade-mcp")
        .with_site(AtkSite::Tr)
        .with_mode(TradingMode::Demo)
        .with_read_only(read_only)
        .with_startup_timeout(Duration::from_secs(1))
        .with_request_timeout(Duration::from_secs(1))
}

fn btc() -> Instrument {
    Instrument::parse("BTC-USDT").unwrap()
}

fn market_data() -> Value {
    json!({
        "code":"0",
        "msg":"",
        "data":[{"instId":"BTC-USDT","last":"60000.10","bidPx":"60000.00","askPx":"60000.20","ts":"1700000000000"}]
    })
}

fn balance_data() -> Value {
    json!({
        "code":"0",
        "msg":"",
        "data":[{"details":[{"ccy":"USDT","cashBal":"2500.00","availBal":"2400.00"}]}]
    })
}

#[test]
fn startup_discovers_capabilities_and_becomes_ready() {
    let transport = ready_transport();
    let client = AtkClient::connect(transport.clone(), config(false)).unwrap();

    assert_eq!(client.health().status, AtkHealth::Ready);
    assert!(client.capabilities().market_data);
    assert!(client.capabilities().account_balance);
    assert!(client.capabilities().spot_order_placement);
    assert!(client.capabilities().spot_order_query);
    assert!(client.capabilities().spot_order_cancellation);
    assert!(!client.capabilities().news);
    assert_eq!(transport.calls()[0].0, "initialize");
    assert_eq!(transport.calls()[1].0, "tools/list");
    assert_eq!(transport.calls()[2].0, "tools/call");
}

#[test]
fn official_nested_capability_snapshot_is_supported() {
    let transport = RecordingTransport::default();
    transport.push(Ok(initialize_response()));
    transport.push(Ok(tools_response(true)));
    transport.push(Ok(nested_capability_response()));

    let client = AtkClient::connect(transport, config(false)).unwrap();

    assert_eq!(client.capabilities().server_version, "1.4.6");
    assert!(client.capabilities().execution_ready);
}

#[test]
fn missing_required_capability_fails_closed() {
    let transport = RecordingTransport::default();
    transport.push(Ok(initialize_response()));
    transport.push(Ok(tools_response(false)));
    transport.push(Ok(capability_response(true, false, Some("tr"))));

    let error = AtkClient::connect(transport, config(false)).unwrap_err();

    assert!(matches!(
        error,
        AtkError::CapabilityUnavailable(RequiredCapability::SpotOrderCancellation)
    ));
}

#[test]
fn malformed_initialize_response_is_a_protocol_error() {
    let transport = RecordingTransport::default();
    transport.push(Ok(
        json!({"jsonrpc":"2.0","id":1,"result":{"capabilities":{}}}),
    ));

    let error = AtkClient::connect(transport, config(false)).unwrap_err();

    assert!(matches!(error, AtkError::ProtocolResponseInvalid(_)));
}

#[test]
fn invalid_tool_schema_is_rejected_during_discovery() {
    let transport = RecordingTransport::default();
    transport.push(Ok(initialize_response()));
    transport.push(Ok(response(
        2,
        json!({"tools":[tool("system_get_capabilities", &[]), tool("market_get_ticker", &[])]}),
    )));

    let error = AtkClient::connect(transport, config(false)).unwrap_err();

    assert!(matches!(error, AtkError::ToolSchemaInvalid { .. }));
}

#[test]
fn mode_mismatch_fails_before_ready() {
    let transport = RecordingTransport::default();
    transport.push(Ok(initialize_response()));
    transport.push(Ok(tools_response(true)));
    transport.push(Ok(capability_response(false, false, Some("tr"))));

    let error = AtkClient::connect(transport, config(false)).unwrap_err();

    assert!(matches!(error, AtkError::ModeMismatch { .. }));
}

#[test]
fn unexpected_server_version_fails_closed() {
    let transport = RecordingTransport::default();
    let mut initialize = initialize_response();
    initialize["result"]["serverInfo"]["version"] = json!("1.4.5");
    transport.push(Ok(initialize));

    let error = AtkClient::connect(transport, config(false)).unwrap_err();

    assert!(matches!(error, AtkError::VersionMismatch { .. }));
}

#[test]
fn site_mismatch_is_reported_by_capability_handshake() {
    let transport = RecordingTransport::default();
    transport.push(Ok(initialize_response()));
    transport.push(Ok(tools_response(true)));
    transport.push(Ok(capability_response(true, false, Some("global"))));

    let error = AtkClient::connect(transport, config(false)).unwrap_err();

    assert!(matches!(error, AtkError::SiteMismatch { .. }));
}

#[test]
fn market_ticker_is_mapped_to_decimal_snapshot() {
    let transport = ready_transport();
    transport.push(Ok(tool_result("market_get_ticker", market_data(), 4)));
    let mut client = AtkClient::connect(transport, config(false)).unwrap();

    let ticker = client.market_ticker(&btc()).unwrap();

    assert_eq!(ticker.last, Decimal::new(6_000_010, 2));
    assert_eq!(ticker.bid, Decimal::new(6_000_000, 2));
    assert_eq!(ticker.ask, Decimal::new(6_000_020, 2));
    assert_eq!(ticker.instrument, btc());
}

#[test]
fn account_balance_is_mapped_without_floating_point() {
    let transport = ready_transport();
    transport.push(Ok(tool_result("account_get_balance", balance_data(), 4)));
    let mut client = AtkClient::connect(transport, config(false)).unwrap();

    let AccountSnapshot { balances, .. } = client.account_balance(None).unwrap();

    assert_eq!(balances[0].asset, "USDT");
    assert_eq!(balances[0].total, Decimal::new(250_000, 2));
    assert_eq!(balances[0].available, Decimal::new(240_000, 2));
}

fn market_order() -> SpotOrderRequest {
    SpotOrderRequest::market(
        btc(),
        OrderSide::Buy,
        Decimal::new(150, 0),
        SizeUnit::Quote,
        "pt-order-1",
    )
    .unwrap()
}

#[test]
fn confirmed_order_preserves_exchange_and_client_identifiers() {
    let transport = ready_transport();
    transport.push(Ok(tool_result(
        "spot_place_order",
        json!({"code":"0","data":[{"ordId":"9001","clOrdId":"pt-order-1","sCode":"0","sMsg":""}]}),
        4,
    )));
    let mut client = AtkClient::connect(transport.clone(), config(false)).unwrap();

    let outcome = client.place_spot_order(&market_order()).unwrap();

    assert_eq!(
        outcome,
        PlacementOutcome::Confirmed {
            order_id: "9001".to_owned(),
            client_order_id: "pt-order-1".to_owned(),
        }
    );
    let calls = transport.calls();
    assert_eq!(calls[3].0, "tools/call");
    assert_eq!(calls[3].1["name"], "spot_place_order");
    assert_eq!(calls[3].1["arguments"]["tdMode"], "cash");
    assert_eq!(calls[3].1["arguments"]["clOrdId"], "pt-order-1");
}

#[test]
fn exchange_rejection_is_not_reported_as_unknown() {
    let transport = ready_transport();
    transport.push(Ok(tool_result(
        "spot_place_order",
        json!({"code":"0","data":[{"ordId":"","clOrdId":"pt-order-1","sCode":"51020","sMsg":"Order quantity invalid"}]}),
        4,
    )));
    let mut client = AtkClient::connect(transport, config(false)).unwrap();

    let outcome = client.place_spot_order(&market_order()).unwrap();

    assert_eq!(
        outcome,
        PlacementOutcome::Rejected {
            code: "51020".to_owned(),
            message: "Order quantity invalid".to_owned(),
        }
    );
}

#[test]
fn placement_timeout_is_unknown_and_is_not_retried() {
    let transport = ready_transport();
    transport.push(Err(McpError::RequestTimeout {
        method: "tools/call".to_owned(),
    }));
    let mut client = AtkClient::connect(transport.clone(), config(false)).unwrap();

    let outcome = client.place_spot_order(&market_order()).unwrap();

    assert_eq!(
        outcome,
        PlacementOutcome::Unknown {
            client_order_id: "pt-order-1".to_owned(),
            reason: "request timeout".to_owned(),
        }
    );
    assert_eq!(transport.calls().len(), 4);
}

#[test]
fn malformed_placement_response_is_unknown() {
    let transport = ready_transport();
    transport.push(Ok(response(
        4,
        json!({"content":[{"type":"text","text":"not-json"}]}),
    )));
    let mut client = AtkClient::connect(transport, config(false)).unwrap();

    let outcome = client.place_spot_order(&market_order()).unwrap();

    assert!(matches!(outcome, PlacementOutcome::Unknown { .. }));
}

#[test]
fn order_query_uses_client_order_id_for_reconciliation() {
    let transport = ready_transport();
    transport.push(Ok(tool_result(
        "spot_get_order",
        json!({"code":"0","data":[{"ordId":"9001","clOrdId":"pt-order-1","state":"filled","accFillSz":"0.002","avgPx":"60010.00"}]}),
        4,
    )));
    let mut client = AtkClient::connect(transport.clone(), config(false)).unwrap();

    let status = client
        .query_order(&OrderQuery::by_client_id(btc(), "pt-order-1"))
        .unwrap();

    assert_eq!(status.state, OrderState::Filled);
    assert_eq!(status.order_id, "9001");
    assert_eq!(status.filled_quantity, Decimal::new(2, 3));
    assert_eq!(transport.calls()[3].1["arguments"]["clOrdId"], "pt-order-1");
}

#[test]
fn cancellation_success_is_explicit() {
    let transport = ready_transport();
    transport.push(Ok(tool_result(
        "spot_cancel_order",
        json!({"code":"0","data":[{"ordId":"9001","clOrdId":"pt-order-1","sCode":"0","sMsg":""}]}),
        4,
    )));
    let mut client = AtkClient::connect(transport, config(false)).unwrap();

    let outcome = client
        .cancel_order(&CancelOrderRequest::by_client_id(btc(), "pt-order-1"))
        .unwrap();

    assert_eq!(
        outcome,
        CancellationOutcome::Confirmed {
            order_id: "9001".to_owned(),
            client_order_id: "pt-order-1".to_owned(),
        }
    );
}

#[test]
fn cancellation_timeout_remains_unknown() {
    let transport = ready_transport();
    transport.push(Err(McpError::RequestTimeout {
        method: "tools/call".to_owned(),
    }));
    let mut client = AtkClient::connect(transport, config(false)).unwrap();

    let outcome = client
        .cancel_order(&CancelOrderRequest::by_client_id(btc(), "pt-order-1"))
        .unwrap();

    assert_eq!(
        outcome,
        CancellationOutcome::Unknown {
            client_order_id: "pt-order-1".to_owned(),
            reason: "request timeout".to_owned(),
        }
    );
}

#[test]
fn process_death_blocks_new_risk_increasing_mutations() {
    let transport = ready_transport();
    let mut client = AtkClient::connect(transport.clone(), config(false)).unwrap();
    transport.mark_dead();

    let error = client.place_spot_order(&market_order()).unwrap_err();

    assert!(matches!(error, AtkError::Unavailable(_)));
    assert_eq!(client.health().status, AtkHealth::Unavailable);
}

#[test]
fn read_only_session_can_read_but_cannot_place_orders() {
    let transport = RecordingTransport::default();
    transport.push(Ok(initialize_response()));
    transport.push(Ok(tools_response(false)));
    transport.push(Ok(capability_response(true, true, Some("tr"))));
    let mut client = AtkClient::connect(transport, config(true)).unwrap();

    assert!(client.capabilities().market_data);
    assert!(!client.capabilities().spot_order_placement);
    let error = client.place_spot_order(&market_order()).unwrap_err();

    assert!(matches!(
        error,
        AtkError::CapabilityUnavailable(RequiredCapability::SpotOrderPlacement)
    ));
}

#[test]
fn live_mutation_requires_explicit_environment_arm() {
    let config = AtkConfig::new("okx-trade-mcp")
        .with_mode(TradingMode::Live)
        .with_read_only(false);

    let error = config.validate().unwrap_err();

    assert!(
        matches!(error, AtkError::InvalidConfig(message) if message.contains("PROOFTRADE_LIVE_ARM"))
    );
}

#[test]
fn invalid_order_request_is_rejected_before_transport_call() {
    let error = SpotOrderRequest::market(
        btc(),
        OrderSide::Buy,
        Decimal::ZERO,
        SizeUnit::Quote,
        "pt-order-1",
    )
    .unwrap_err();

    assert!(matches!(error, AtkError::InvalidOrderRequest(_)));
}

#[test]
fn status_parser_preserves_unknown_exchange_states() {
    let status = OrderStatus::from_exchange("some_future_state").unwrap();

    assert_eq!(status, OrderState::Unknown("some_future_state".to_owned()));
}
