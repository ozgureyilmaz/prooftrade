mod error;
mod transport;

pub use error::{AtkError, McpError};
pub use transport::{McpTransport, StdioTransport};

use std::collections::{BTreeMap, BTreeSet};
use std::env;
use std::path::PathBuf;
use std::process::Command;
use std::str::FromStr;
use std::time::Duration;

use rust_decimal::Decimal;
use serde_json::{Map, Value, json};
use time::OffsetDateTime;

use crate::domain::Instrument;

pub const OFFICIAL_REPOSITORY: &str = "https://github.com/okx/agent-trade-kit";
pub const OFFICIAL_PACKAGE: &str = "@okx_ai/okx-trade-mcp";
pub const VERIFIED_VERSION: &str = "1.4.6";
pub const OFFICIAL_EXECUTABLE: &str = "okx-trade-mcp";

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum AtkSite {
    Global,
    Eea,
    Us,
    Tr,
}

impl AtkSite {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Global => "global",
            Self::Eea => "eea",
            Self::Us => "us",
            Self::Tr => "tr",
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum TradingMode {
    Demo,
    Live,
}

impl TradingMode {
    const fn is_demo(self) -> bool {
        matches!(self, Self::Demo)
    }
}

#[derive(Clone, Debug)]
pub struct AtkConfig {
    pub executable: PathBuf,
    pub profile: Option<String>,
    pub site: AtkSite,
    pub mode: TradingMode,
    pub read_only: bool,
    pub modules: Vec<String>,
    pub expected_protocol_version: Option<String>,
    pub expected_server_version: Option<String>,
    pub startup_timeout: Duration,
    pub request_timeout: Duration,
    pub shutdown_timeout: Duration,
}

impl AtkConfig {
    pub fn new(executable: impl Into<PathBuf>) -> Self {
        Self {
            executable: executable.into(),
            profile: None,
            site: AtkSite::Tr,
            mode: TradingMode::Demo,
            read_only: true,
            modules: vec!["market".to_owned(), "spot".to_owned(), "account".to_owned()],
            expected_protocol_version: None,
            expected_server_version: Some(VERIFIED_VERSION.to_owned()),
            startup_timeout: Duration::from_secs(10),
            request_timeout: Duration::from_secs(15),
            shutdown_timeout: Duration::from_secs(5),
        }
    }

    pub fn with_site(mut self, site: AtkSite) -> Self {
        self.site = site;
        self
    }

    pub fn with_mode(mut self, mode: TradingMode) -> Self {
        self.mode = mode;
        self
    }

    pub fn with_read_only(mut self, read_only: bool) -> Self {
        self.read_only = read_only;
        self
    }

    pub fn with_profile(mut self, profile: impl Into<String>) -> Self {
        self.profile = Some(profile.into());
        self
    }

    pub fn with_modules(mut self, modules: impl IntoIterator<Item = impl Into<String>>) -> Self {
        self.modules = modules.into_iter().map(Into::into).collect();
        self
    }

    pub fn with_startup_timeout(mut self, timeout: Duration) -> Self {
        self.startup_timeout = timeout;
        self
    }

    pub fn with_request_timeout(mut self, timeout: Duration) -> Self {
        self.request_timeout = timeout;
        self
    }

    pub fn with_shutdown_timeout(mut self, timeout: Duration) -> Self {
        self.shutdown_timeout = timeout;
        self
    }

    pub fn with_expected_protocol_version(mut self, version: impl Into<String>) -> Self {
        self.expected_protocol_version = Some(version.into());
        self
    }

    pub fn with_expected_server_version(mut self, version: impl Into<String>) -> Self {
        self.expected_server_version = Some(version.into());
        self
    }

    pub fn validate(&self) -> Result<(), AtkError> {
        if self.executable.as_os_str().is_empty() {
            return Err(AtkError::InvalidConfig(
                "executable cannot be empty".to_owned(),
            ));
        }
        if self.modules.is_empty() {
            return Err(AtkError::InvalidConfig(
                "at least one module is required".to_owned(),
            ));
        }
        if self.startup_timeout.is_zero()
            || self.request_timeout.is_zero()
            || self.shutdown_timeout.is_zero()
        {
            return Err(AtkError::InvalidConfig(
                "timeouts must be positive".to_owned(),
            ));
        }
        if matches!(self.mode, TradingMode::Live)
            && !self.read_only
            && env::var("PROOFTRADE_LIVE_ARM").ok().as_deref() != Some("1")
        {
            return Err(AtkError::InvalidConfig(
                "live mutation requires PROOFTRADE_LIVE_ARM=1".to_owned(),
            ));
        }
        Ok(())
    }

    pub fn command(&self) -> Command {
        let mut command = Command::new(&self.executable);
        command.arg("--modules").arg(self.modules.join(","));
        command.arg("--site").arg(self.site.as_str());
        if let Some(profile) = &self.profile {
            command.arg("--profile").arg(profile);
        }
        if self.read_only {
            command.arg("--read-only");
        }
        if self.mode.is_demo() {
            command.arg("--demo");
        } else {
            command.arg("--live");
        }
        command
    }
}

#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub enum RequiredCapability {
    MarketData,
    AccountBalance,
    SpotOrderPlacement,
    SpotOrderQuery,
    SpotOrderCancellation,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum AtkHealth {
    Starting,
    Ready,
    Degraded,
    Unavailable,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum SiteVerification {
    Verified,
    Unreported,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct HealthReport {
    pub status: AtkHealth,
    pub reason: Option<String>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct CapabilityReport {
    pub server_version: String,
    pub tools: BTreeSet<String>,
    pub market_data: bool,
    pub account_balance: bool,
    pub spot_order_placement: bool,
    pub spot_order_query: bool,
    pub spot_order_cancellation: bool,
    pub news: bool,
    pub read_only: bool,
    pub demo: bool,
    pub has_auth: bool,
    pub site_verification: SiteVerification,
    pub execution_ready: bool,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct MarketSnapshot {
    pub instrument: Instrument,
    pub bid: Decimal,
    pub ask: Decimal,
    pub last: Decimal,
    pub observed_at: OffsetDateTime,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct AssetBalance {
    pub asset: String,
    pub total: Decimal,
    pub available: Decimal,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct AccountSnapshot {
    pub balances: Vec<AssetBalance>,
    pub observed_at: OffsetDateTime,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum OrderSide {
    Buy,
    Sell,
}

impl OrderSide {
    const fn as_str(self) -> &'static str {
        match self {
            Self::Buy => "buy",
            Self::Sell => "sell",
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum SizeUnit {
    Base,
    Quote,
}

impl SizeUnit {
    const fn as_str(self) -> &'static str {
        match self {
            Self::Base => "base_ccy",
            Self::Quote => "quote_ccy",
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum OrderType {
    Market,
    Limit,
}

impl OrderType {
    const fn as_str(self) -> &'static str {
        match self {
            Self::Market => "market",
            Self::Limit => "limit",
        }
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct SpotOrderRequest {
    pub instrument: Instrument,
    pub side: OrderSide,
    pub size: Decimal,
    pub size_unit: SizeUnit,
    pub price: Option<Decimal>,
    pub client_order_id: String,
    order_type: OrderType,
}

impl SpotOrderRequest {
    pub fn market(
        instrument: Instrument,
        side: OrderSide,
        size: Decimal,
        size_unit: SizeUnit,
        client_order_id: impl Into<String>,
    ) -> Result<Self, AtkError> {
        Self::build(
            instrument,
            side,
            size,
            size_unit,
            None,
            client_order_id.into(),
            OrderType::Market,
        )
    }

    pub fn limit(
        instrument: Instrument,
        side: OrderSide,
        size: Decimal,
        size_unit: SizeUnit,
        price: Decimal,
        client_order_id: impl Into<String>,
    ) -> Result<Self, AtkError> {
        Self::build(
            instrument,
            side,
            size,
            size_unit,
            Some(price),
            client_order_id.into(),
            OrderType::Limit,
        )
    }

    fn build(
        instrument: Instrument,
        side: OrderSide,
        size: Decimal,
        size_unit: SizeUnit,
        price: Option<Decimal>,
        client_order_id: String,
        order_type: OrderType,
    ) -> Result<Self, AtkError> {
        if size <= Decimal::ZERO {
            return Err(AtkError::InvalidOrderRequest(
                "size must be greater than zero".to_owned(),
            ));
        }
        if order_type == OrderType::Limit && price.is_none_or(|value| value <= Decimal::ZERO) {
            return Err(AtkError::InvalidOrderRequest(
                "limit price must be greater than zero".to_owned(),
            ));
        }
        if client_order_id.trim().is_empty() {
            return Err(AtkError::InvalidOrderRequest(
                "client order ID is required".to_owned(),
            ));
        }
        if client_order_id.len() > 32 {
            return Err(AtkError::InvalidOrderRequest(
                "client order ID cannot exceed 32 bytes".to_owned(),
            ));
        }
        if client_order_id.chars().any(char::is_whitespace) {
            return Err(AtkError::InvalidOrderRequest(
                "client order ID cannot contain whitespace".to_owned(),
            ));
        }
        instrument
            .validate()
            .map_err(|error| AtkError::InvalidOrderRequest(error.to_string()))?;
        Ok(Self {
            instrument,
            side,
            size,
            size_unit,
            price,
            client_order_id,
            order_type,
        })
    }

    fn arguments(&self) -> Value {
        let mut arguments = Map::new();
        arguments.insert("instId".to_owned(), json!(self.instrument.symbol()));
        arguments.insert("tdMode".to_owned(), json!("cash"));
        arguments.insert("side".to_owned(), json!(self.side.as_str()));
        arguments.insert("ordType".to_owned(), json!(self.order_type.as_str()));
        arguments.insert("sz".to_owned(), json!(self.size.to_string()));
        arguments.insert("tgtCcy".to_owned(), json!(self.size_unit.as_str()));
        arguments.insert("clOrdId".to_owned(), json!(self.client_order_id));
        if let Some(price) = self.price {
            arguments.insert("px".to_owned(), json!(price.to_string()));
        }
        Value::Object(arguments)
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum PlacementOutcome {
    Confirmed {
        order_id: String,
        client_order_id: String,
    },
    Rejected {
        code: String,
        message: String,
    },
    Unknown {
        client_order_id: String,
        reason: String,
    },
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct OrderQuery {
    pub instrument: Instrument,
    pub order_id: Option<String>,
    pub client_order_id: Option<String>,
}

impl OrderQuery {
    pub fn by_client_id(instrument: Instrument, client_order_id: impl Into<String>) -> Self {
        Self {
            instrument,
            order_id: None,
            client_order_id: Some(client_order_id.into()),
        }
    }

    pub fn by_order_id(instrument: Instrument, order_id: impl Into<String>) -> Self {
        Self {
            instrument,
            order_id: Some(order_id.into()),
            client_order_id: None,
        }
    }

    fn arguments(&self) -> Result<Value, AtkError> {
        let mut arguments = Map::new();
        arguments.insert("instId".to_owned(), json!(self.instrument.symbol()));
        match (&self.order_id, &self.client_order_id) {
            (Some(order_id), None) => {
                arguments.insert("ordId".to_owned(), json!(order_id));
            }
            (None, Some(client_order_id)) => {
                arguments.insert("clOrdId".to_owned(), json!(client_order_id));
            }
            _ => {
                return Err(AtkError::InvalidOrderRequest(
                    "exactly one order identifier is required".to_owned(),
                ));
            }
        }
        Ok(Value::Object(arguments))
    }

    fn validate(&self) -> Result<(), AtkError> {
        self.instrument
            .validate()
            .map_err(|error| AtkError::InvalidOrderRequest(error.to_string()))?;
        match (&self.order_id, &self.client_order_id) {
            (Some(identifier), None) | (None, Some(identifier))
                if !identifier.trim().is_empty()
                    && !identifier.chars().any(char::is_whitespace) =>
            {
                Ok(())
            }
            _ => Err(AtkError::InvalidOrderRequest(
                "exactly one non-empty order identifier is required".to_owned(),
            )),
        }
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct CancelOrderRequest {
    pub instrument: Instrument,
    pub order_id: Option<String>,
    pub client_order_id: Option<String>,
}

impl CancelOrderRequest {
    pub fn by_client_id(instrument: Instrument, client_order_id: impl Into<String>) -> Self {
        Self {
            instrument,
            order_id: None,
            client_order_id: Some(client_order_id.into()),
        }
    }

    pub fn by_order_id(instrument: Instrument, order_id: impl Into<String>) -> Self {
        Self {
            instrument,
            order_id: Some(order_id.into()),
            client_order_id: None,
        }
    }

    fn arguments(&self) -> Result<Value, AtkError> {
        OrderQuery {
            instrument: self.instrument.clone(),
            order_id: self.order_id.clone(),
            client_order_id: self.client_order_id.clone(),
        }
        .arguments()
    }

    fn validate(&self) -> Result<(), AtkError> {
        OrderQuery {
            instrument: self.instrument.clone(),
            order_id: self.order_id.clone(),
            client_order_id: self.client_order_id.clone(),
        }
        .validate()
    }

    fn client_id(&self) -> Option<String> {
        self.client_order_id.clone()
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum CancellationOutcome {
    Confirmed {
        order_id: String,
        client_order_id: String,
    },
    Rejected {
        code: String,
        message: String,
    },
    Unknown {
        client_order_id: String,
        reason: String,
    },
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum OrderState {
    Open,
    PartiallyFilled,
    Filled,
    Canceled,
    Failed,
    Unknown(String),
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct OrderStatus {
    pub order_id: String,
    pub client_order_id: String,
    pub state: OrderState,
    pub filled_quantity: Decimal,
    pub average_price: Option<Decimal>,
}

impl OrderStatus {
    pub fn from_exchange(value: &str) -> Result<OrderState, AtkError> {
        Ok(match value {
            "live" => OrderState::Open,
            "partially_filled" => OrderState::PartiallyFilled,
            "filled" => OrderState::Filled,
            "canceled" | "mmp_canceled" => OrderState::Canceled,
            "order_failed" => OrderState::Failed,
            other if !other.trim().is_empty() => OrderState::Unknown(other.to_owned()),
            _ => return Err(AtkError::ResponseInvalid("order state is empty".to_owned())),
        })
    }
}

#[derive(Debug)]
pub struct AtkClient<T> {
    transport: T,
    config: AtkConfig,
    health: HealthReport,
    capabilities: CapabilityReport,
}

impl<T: McpTransport> AtkClient<T> {
    pub fn connect(transport: T, config: AtkConfig) -> Result<Self, AtkError> {
        config.validate()?;
        if !transport.is_alive() {
            return Err(AtkError::Unavailable("MCP process is not alive".to_owned()));
        }
        let mut client = Self {
            transport,
            config,
            health: HealthReport {
                status: AtkHealth::Starting,
                reason: None,
            },
            capabilities: CapabilityReport {
                server_version: String::new(),
                tools: BTreeSet::new(),
                market_data: false,
                account_balance: false,
                spot_order_placement: false,
                spot_order_query: false,
                spot_order_cancellation: false,
                news: false,
                read_only: false,
                demo: false,
                has_auth: false,
                site_verification: SiteVerification::Unreported,
                execution_ready: false,
            },
        };
        if let Err(error) = client.startup() {
            client.health = HealthReport {
                status: AtkHealth::Unavailable,
                reason: Some(error.to_string()),
            };
            return Err(error);
        }
        Ok(client)
    }

    pub fn health(&self) -> &HealthReport {
        &self.health
    }

    pub fn capabilities(&self) -> &CapabilityReport {
        &self.capabilities
    }

    pub fn trading_mode(&self) -> TradingMode {
        self.config.mode
    }

    pub fn market_ticker(&mut self, instrument: &Instrument) -> Result<MarketSnapshot, AtkError> {
        instrument
            .validate()
            .map_err(|error| AtkError::InvalidOrderRequest(error.to_string()))?;
        self.ensure_read(RequiredCapability::MarketData)?;
        let payload =
            self.call_tool("market_get_ticker", json!({"instId": instrument.symbol()}))?;
        parse_market_snapshot(&payload, instrument)
    }

    pub fn account_balance(&mut self, currency: Option<&str>) -> Result<AccountSnapshot, AtkError> {
        self.ensure_read(RequiredCapability::AccountBalance)?;
        let arguments = currency.map_or_else(|| json!({}), |ccy| json!({"ccy": ccy}));
        let payload = self.call_tool("account_get_balance", arguments)?;
        parse_account_snapshot(&payload)
    }

    pub fn place_spot_order(
        &mut self,
        request: &SpotOrderRequest,
    ) -> Result<PlacementOutcome, AtkError> {
        request
            .instrument
            .validate()
            .map_err(|error| AtkError::InvalidOrderRequest(error.to_string()))?;
        self.ensure_mutation(RequiredCapability::SpotOrderPlacement)?;
        let client_order_id = request.client_order_id.clone();
        match self.call_tool_result("spot_place_order", request.arguments()) {
            Ok(Ok(payload)) => match parse_placement(&payload) {
                Ok(outcome) => Ok(outcome),
                Err(error) => Ok(PlacementOutcome::Unknown {
                    client_order_id,
                    reason: format!("invalid placement response: {error}"),
                }),
            },
            Ok(Err(rejection)) => Ok(PlacementOutcome::Rejected {
                code: rejection.0,
                message: rejection.1,
            }),
            Err(error) => Ok(PlacementOutcome::Unknown {
                client_order_id,
                reason: mutation_unknown_reason(&error),
            }),
        }
    }

    pub fn query_order(&mut self, query: &OrderQuery) -> Result<OrderStatus, AtkError> {
        query.validate()?;
        self.ensure_read(RequiredCapability::SpotOrderQuery)?;
        let payload = self.call_tool("spot_get_order", query.arguments()?)?;
        parse_order_status(&payload)
    }

    pub fn cancel_order(
        &mut self,
        request: &CancelOrderRequest,
    ) -> Result<CancellationOutcome, AtkError> {
        request.validate()?;
        self.ensure_mutation(RequiredCapability::SpotOrderCancellation)?;
        let client_order_id = request
            .client_id()
            .unwrap_or_else(|| "unknown-client-order".to_owned());
        match self.call_tool_result("spot_cancel_order", request.arguments()?) {
            Ok(Ok(payload)) => match parse_cancellation(&payload) {
                Ok(outcome) => Ok(outcome),
                Err(error) => Ok(CancellationOutcome::Unknown {
                    client_order_id,
                    reason: format!("invalid cancellation response: {error}"),
                }),
            },
            Ok(Err(rejection)) => Ok(CancellationOutcome::Rejected {
                code: rejection.0,
                message: rejection.1,
            }),
            Err(error) => Ok(CancellationOutcome::Unknown {
                client_order_id,
                reason: mutation_unknown_reason(&error),
            }),
        }
    }

    pub fn shutdown(&mut self) -> Result<(), AtkError> {
        self.transport.shutdown().map_err(AtkError::Transport)?;
        self.health = HealthReport {
            status: AtkHealth::Unavailable,
            reason: Some("shutdown requested".to_owned()),
        };
        Ok(())
    }

    fn startup(&mut self) -> Result<(), AtkError> {
        let initialize = self.request_rpc_with_timeout(
            "initialize",
            json!({
                "protocolVersion": "2025-06-18",
                "capabilities": {},
                "clientInfo": {"name":"prooftrade","version":"0.1.0"}
            }),
            self.config.startup_timeout,
        )?;
        let protocol_version = initialize
            .get("protocolVersion")
            .and_then(Value::as_str)
            .filter(|value| !value.trim().is_empty())
            .ok_or_else(|| {
                AtkError::ProtocolResponseInvalid(
                    "initialize.protocolVersion is missing".to_owned(),
                )
            })?;
        if self
            .config
            .expected_protocol_version
            .as_deref()
            .is_some_and(|expected| expected != protocol_version)
        {
            return Err(AtkError::ProtocolResponseInvalid(format!(
                "expected protocol version {:?}, got {protocol_version}",
                self.config.expected_protocol_version
            )));
        }
        let server_version = initialize
            .get("serverInfo")
            .and_then(|value| value.get("version"))
            .and_then(Value::as_str)
            .filter(|value| !value.trim().is_empty())
            .ok_or_else(|| {
                AtkError::ProtocolResponseInvalid(
                    "initialize.serverInfo.version is missing".to_owned(),
                )
            })?;
        if self
            .config
            .expected_server_version
            .as_deref()
            .is_some_and(|expected| expected != server_version)
        {
            return Err(AtkError::VersionMismatch {
                expected: self
                    .config
                    .expected_server_version
                    .clone()
                    .unwrap_or_default(),
                actual: server_version.to_owned(),
            });
        }
        self.capabilities.server_version = server_version.to_owned();
        self.transport
            .notify("notifications/initialized", json!({}))
            .map_err(AtkError::Transport)?;

        let tools_result =
            self.request_rpc_with_timeout("tools/list", json!({}), self.config.startup_timeout)?;
        let tools = parse_tools(&tools_result)?;
        validate_tool_schemas(&tools)?;
        self.capabilities.tools = tools.keys().cloned().collect();
        self.capabilities.news = self
            .capabilities
            .tools
            .iter()
            .any(|name| name.starts_with("news_"));

        let capability_payload = self.call_tool_with_timeout(
            "system_get_capabilities",
            json!({}),
            self.config.startup_timeout,
        )?;
        let snapshot = capability_payload.get("data").ok_or_else(|| {
            AtkError::ResponseInvalid("capability snapshot data is missing".to_owned())
        })?;
        // Current official ATK wraps the runtime snapshot in
        // data.capabilities. The direct shape is retained for small fixtures.
        let runtime_snapshot = snapshot.get("capabilities").unwrap_or(snapshot);
        let actual_demo = runtime_snapshot
            .get("demo")
            .and_then(Value::as_bool)
            .ok_or_else(|| {
                AtkError::ResponseInvalid("capability snapshot demo is missing".to_owned())
            })?;
        let actual_read_only = runtime_snapshot
            .get("readOnly")
            .and_then(Value::as_bool)
            .ok_or_else(|| {
                AtkError::ResponseInvalid("capability snapshot readOnly is missing".to_owned())
            })?;
        if actual_demo != self.config.mode.is_demo() {
            return Err(AtkError::ModeMismatch {
                expected_demo: self.config.mode.is_demo(),
                actual_demo,
            });
        }
        if actual_read_only != self.config.read_only {
            return Err(AtkError::ReadOnlyMismatch {
                expected: self.config.read_only,
                actual: actual_read_only,
            });
        }
        let site_verification = match runtime_snapshot.get("site").and_then(Value::as_str) {
            Some(actual) if actual != self.config.site.as_str() => {
                return Err(AtkError::SiteMismatch {
                    expected: self.config.site.as_str().to_owned(),
                    actual: actual.to_owned(),
                });
            }
            Some(_) => SiteVerification::Verified,
            None => SiteVerification::Unreported,
        };
        let has_auth = runtime_snapshot
            .get("hasAuth")
            .and_then(Value::as_bool)
            .unwrap_or(false);
        if matches!(self.config.mode, TradingMode::Live) && !self.config.read_only && !has_auth {
            return Err(AtkError::InvalidConfig(
                "live mutation requires authenticated ATK capabilities".to_owned(),
            ));
        }
        let module_available = |module: &str| {
            runtime_snapshot
                .get("moduleAvailability")
                .and_then(|value| value.get(module))
                .and_then(|value| value.get("status"))
                .and_then(Value::as_str)
                .is_some_and(|status| status == "enabled")
        };
        self.capabilities.market_data =
            tools.contains_key("market_get_ticker") && module_available("market");
        self.capabilities.account_balance =
            tools.contains_key("account_get_balance") && module_available("account");
        self.capabilities.spot_order_query =
            tools.contains_key("spot_get_order") && module_available("spot");
        self.capabilities.spot_order_placement = !self.config.read_only
            && tools.contains_key("spot_place_order")
            && module_available("spot");
        self.capabilities.spot_order_cancellation = !self.config.read_only
            && tools.contains_key("spot_cancel_order")
            && module_available("spot");
        self.capabilities.read_only = actual_read_only;
        self.capabilities.demo = actual_demo;
        self.capabilities.has_auth = has_auth;
        self.capabilities.site_verification = site_verification.clone();
        let required = [
            (
                RequiredCapability::MarketData,
                self.capabilities.market_data,
            ),
            (
                RequiredCapability::AccountBalance,
                self.capabilities.account_balance,
            ),
            (
                RequiredCapability::SpotOrderQuery,
                self.capabilities.spot_order_query,
            ),
        ];
        for (capability, available) in required {
            if !available {
                return Err(AtkError::CapabilityUnavailable(capability));
            }
        }
        if !self.config.read_only {
            for (capability, available) in [
                (
                    RequiredCapability::SpotOrderPlacement,
                    self.capabilities.spot_order_placement,
                ),
                (
                    RequiredCapability::SpotOrderCancellation,
                    self.capabilities.spot_order_cancellation,
                ),
            ] {
                if !available {
                    return Err(AtkError::CapabilityUnavailable(capability));
                }
            }
        }
        self.capabilities.execution_ready = self.capabilities.market_data
            && self.capabilities.account_balance
            && self.capabilities.spot_order_query
            && self.capabilities.spot_order_placement
            && self.capabilities.spot_order_cancellation;
        self.health = HealthReport {
            status: AtkHealth::Ready,
            reason: (site_verification == SiteVerification::Unreported)
                .then(|| "ATK did not report its site in the capability snapshot".to_owned()),
        };
        Ok(())
    }

    fn ensure_read(&mut self, capability: RequiredCapability) -> Result<(), AtkError> {
        self.ensure_available()?;
        let available = match capability {
            RequiredCapability::MarketData => self.capabilities.market_data,
            RequiredCapability::AccountBalance => self.capabilities.account_balance,
            RequiredCapability::SpotOrderQuery => self.capabilities.spot_order_query,
            RequiredCapability::SpotOrderPlacement => self.capabilities.spot_order_placement,
            RequiredCapability::SpotOrderCancellation => self.capabilities.spot_order_cancellation,
        };
        if !available {
            return Err(AtkError::CapabilityUnavailable(capability));
        }
        Ok(())
    }

    fn ensure_mutation(&mut self, capability: RequiredCapability) -> Result<(), AtkError> {
        self.ensure_read(capability)
    }

    fn ensure_available(&mut self) -> Result<(), AtkError> {
        if !self.transport.is_alive() {
            self.health = HealthReport {
                status: AtkHealth::Unavailable,
                reason: Some("MCP process is not alive".to_owned()),
            };
            return Err(AtkError::Unavailable("MCP process is not alive".to_owned()));
        }
        if self.health.status != AtkHealth::Ready {
            return Err(AtkError::NotReady);
        }
        Ok(())
    }

    fn request_rpc_with_timeout(
        &self,
        method: &str,
        params: Value,
        timeout: Duration,
    ) -> Result<Value, AtkError> {
        let response = self
            .transport
            .request(method, params, timeout)
            .map_err(AtkError::Transport)?;
        if response.get("jsonrpc").and_then(Value::as_str) != Some("2.0") {
            return Err(AtkError::ProtocolResponseInvalid(
                "MCP response.jsonrpc must be 2.0".to_owned(),
            ));
        }
        if let Some(error) = response.get("error") {
            let code = error.get("code").and_then(Value::as_i64).unwrap_or(-1);
            let message = error
                .get("message")
                .and_then(Value::as_str)
                .unwrap_or("unknown JSON-RPC error")
                .to_owned();
            return Err(AtkError::ProtocolError { code, message });
        }
        response.get("result").cloned().ok_or_else(|| {
            AtkError::ProtocolResponseInvalid("MCP response.result is missing".to_owned())
        })
    }

    fn call_tool(&self, name: &str, arguments: Value) -> Result<Value, AtkError> {
        self.call_tool_with_timeout(name, arguments, self.config.request_timeout)
    }

    fn call_tool_with_timeout(
        &self,
        name: &str,
        arguments: Value,
        timeout: Duration,
    ) -> Result<Value, AtkError> {
        match self.call_tool_result_with_timeout(name, arguments, timeout)? {
            Ok(payload) => Ok(payload),
            Err((code, message)) => Err(AtkError::ToolRejected {
                tool: name.to_owned(),
                code,
                message,
            }),
        }
    }

    fn call_tool_result(
        &self,
        name: &str,
        arguments: Value,
    ) -> Result<Result<Value, (String, String)>, AtkError> {
        self.call_tool_result_with_timeout(name, arguments, self.config.request_timeout)
    }

    fn call_tool_result_with_timeout(
        &self,
        name: &str,
        arguments: Value,
        timeout: Duration,
    ) -> Result<Result<Value, (String, String)>, AtkError> {
        let result = self.request_rpc_with_timeout(
            "tools/call",
            json!({"name": name, "arguments": arguments}),
            timeout,
        )?;
        let is_error = result
            .get("isError")
            .and_then(Value::as_bool)
            .unwrap_or(false);
        let payload =
            if result.get("structuredContent").is_some() || result.get("content").is_some() {
                parse_tool_payload(&result)?
            } else {
                result
            };
        if is_error || payload.get("ok").and_then(Value::as_bool) == Some(false) {
            return Ok(Err(tool_rejection(&payload)));
        }
        Ok(Ok(payload))
    }
}

impl AtkClient<StdioTransport> {
    pub fn launch(config: AtkConfig) -> Result<Self, AtkError> {
        config.validate()?;
        let mut command = config.command();
        let transport = StdioTransport::spawn(&mut command)
            .map_err(|error| AtkError::LaunchFailed(error.to_string()))?;
        Self::connect(transport, config)
    }
}

fn parse_tools(result: &Value) -> Result<BTreeMap<String, Value>, AtkError> {
    let tools = result
        .get("tools")
        .and_then(Value::as_array)
        .ok_or_else(|| {
            AtkError::ProtocolResponseInvalid("tools/list result.tools is missing".to_owned())
        })?;
    let mut parsed = BTreeMap::new();
    for tool in tools {
        let name = tool
            .get("name")
            .and_then(Value::as_str)
            .filter(|name| !name.trim().is_empty())
            .ok_or_else(|| AtkError::ToolSchemaInvalid {
                tool: "<unnamed>".to_owned(),
                reason: "name is missing".to_owned(),
            })?;
        parsed.insert(name.to_owned(), tool.clone());
    }
    Ok(parsed)
}

fn validate_tool_schemas(tools: &BTreeMap<String, Value>) -> Result<(), AtkError> {
    let schemas = [
        ("system_get_capabilities", &[][..]),
        ("market_get_ticker", &["instId"][..]),
        ("account_get_balance", &[][..]),
        (
            "spot_place_order",
            &["instId", "tdMode", "side", "ordType", "sz"][..],
        ),
        ("spot_get_order", &["instId"][..]),
        ("spot_cancel_order", &["instId"][..]),
    ];
    for (name, required) in schemas {
        let Some(tool) = tools.get(name) else {
            continue;
        };
        let Some(schema) = tool.get("inputSchema").and_then(Value::as_object) else {
            return Err(AtkError::ToolSchemaInvalid {
                tool: name.to_owned(),
                reason: "inputSchema must be an object".to_owned(),
            });
        };
        if schema.get("type").and_then(Value::as_str) != Some("object") {
            return Err(AtkError::ToolSchemaInvalid {
                tool: name.to_owned(),
                reason: "inputSchema.type must be object".to_owned(),
            });
        }
        let properties = schema.get("properties").and_then(Value::as_object);
        for field in required {
            if !properties.is_some_and(|properties| properties.contains_key(*field)) {
                return Err(AtkError::ToolSchemaInvalid {
                    tool: name.to_owned(),
                    reason: format!("required property {field} is missing"),
                });
            }
        }
    }
    Ok(())
}

fn parse_tool_payload(result: &Value) -> Result<Value, AtkError> {
    if let Some(payload) = result.get("structuredContent") {
        if payload.is_object() {
            return Ok(payload.clone());
        }
    }
    let text = result
        .get("content")
        .and_then(Value::as_array)
        .and_then(|content| {
            content
                .iter()
                .find_map(|item| item.get("text").and_then(Value::as_str))
        })
        .ok_or_else(|| {
            AtkError::ResponseInvalid("tool result contains no structured content".to_owned())
        })?;
    serde_json::from_str(text).map_err(|error| AtkError::ResponseInvalid(error.to_string()))
}

fn tool_rejection(payload: &Value) -> (String, String) {
    let code = payload
        .get("code")
        .or_else(|| payload.get("error").and_then(|value| value.get("code")))
        .and_then(Value::as_str)
        .unwrap_or("ATK_TOOL_ERROR")
        .to_owned();
    let message = payload
        .get("message")
        .or_else(|| payload.get("error").and_then(|value| value.get("message")))
        .and_then(Value::as_str)
        .unwrap_or("ATK tool rejected the request")
        .to_owned();
    (code, message)
}

fn nested_data(payload: &Value) -> Result<&Value, AtkError> {
    payload
        .get("data")
        .and_then(|value| value.get("data").or(Some(value)))
        .ok_or_else(|| AtkError::ResponseInvalid("tool data is missing".to_owned()))
}

fn first_record(payload: &Value) -> Result<&Map<String, Value>, AtkError> {
    let data = nested_data(payload)?;
    if let Some(record) = data.as_object() {
        return Ok(record);
    }
    data.as_array()
        .and_then(|items| items.first())
        .and_then(Value::as_object)
        .ok_or_else(|| AtkError::ResponseInvalid("tool data must contain an object".to_owned()))
}

fn parse_market_snapshot(
    payload: &Value,
    requested: &Instrument,
) -> Result<MarketSnapshot, AtkError> {
    let record = first_record(payload)?;
    let actual = record
        .get("instId")
        .and_then(Value::as_str)
        .ok_or_else(|| AtkError::ResponseInvalid("market ticker instId is missing".to_owned()))?;
    let instrument = Instrument::parse(actual).map_err(|error| {
        AtkError::ResponseInvalid(format!("invalid market instrument: {error}"))
    })?;
    if &instrument != requested {
        return Err(AtkError::ResponseInvalid(
            "market instrument does not match request".to_owned(),
        ));
    }
    Ok(MarketSnapshot {
        instrument,
        bid: decimal_field(record, "bidPx")?,
        ask: decimal_field(record, "askPx")?,
        last: decimal_field(record, "last")?,
        observed_at: timestamp_field(record.get("ts"), payload.get("timestamp"))?,
    })
}

fn parse_account_snapshot(payload: &Value) -> Result<AccountSnapshot, AtkError> {
    let account = first_record(payload)?;
    let details = account
        .get("details")
        .and_then(Value::as_array)
        .ok_or_else(|| {
            AtkError::ResponseInvalid("account balance details are missing".to_owned())
        })?;
    let mut balances = Vec::with_capacity(details.len());
    for detail in details {
        let record = detail.as_object().ok_or_else(|| {
            AtkError::ResponseInvalid("account balance detail is not an object".to_owned())
        })?;
        let asset = record
            .get("ccy")
            .and_then(Value::as_str)
            .filter(|value| !value.trim().is_empty())
            .ok_or_else(|| {
                AtkError::ResponseInvalid("account balance currency is missing".to_owned())
            })?;
        balances.push(AssetBalance {
            asset: asset.to_owned(),
            total: decimal_field(record, "cashBal").or_else(|_| decimal_field(record, "eq"))?,
            available: decimal_field(record, "availBal")
                .or_else(|_| decimal_field(record, "availEq"))?,
        });
    }
    Ok(AccountSnapshot {
        balances,
        observed_at: timestamp_field(None, payload.get("timestamp"))?,
    })
}

fn parse_placement(payload: &Value) -> Result<PlacementOutcome, AtkError> {
    let record = first_record(payload)?;
    let code = record.get("sCode").and_then(Value::as_str).unwrap_or("0");
    let message = record.get("sMsg").and_then(Value::as_str).unwrap_or("");
    let client_order_id = record
        .get("clOrdId")
        .and_then(Value::as_str)
        .filter(|value| !value.is_empty())
        .ok_or_else(|| AtkError::ResponseInvalid("placement clOrdId is missing".to_owned()))?;
    if code != "0" {
        return Ok(PlacementOutcome::Rejected {
            code: code.to_owned(),
            message: message.to_owned(),
        });
    }
    let order_id = record
        .get("ordId")
        .and_then(Value::as_str)
        .filter(|value| !value.is_empty())
        .ok_or_else(|| AtkError::ResponseInvalid("placement ordId is missing".to_owned()))?;
    Ok(PlacementOutcome::Confirmed {
        order_id: order_id.to_owned(),
        client_order_id: client_order_id.to_owned(),
    })
}

fn parse_order_status(payload: &Value) -> Result<OrderStatus, AtkError> {
    let record = first_record(payload)?;
    let order_id = string_field(record, "ordId")?;
    let client_order_id = optional_string_field(record, "clOrdId");
    let state = OrderStatus::from_exchange(&string_field(record, "state")?)?;
    let filled_quantity = decimal_field(record, "accFillSz")?;
    let average_price = record
        .get("avgPx")
        .filter(|value| value.as_str() != Some(""))
        .map(|value| decimal_value(value, "avgPx"))
        .transpose()?;
    Ok(OrderStatus {
        order_id,
        client_order_id,
        state,
        filled_quantity,
        average_price,
    })
}

fn parse_cancellation(payload: &Value) -> Result<CancellationOutcome, AtkError> {
    let record = first_record(payload)?;
    let code = record.get("sCode").and_then(Value::as_str).unwrap_or("0");
    let message = record.get("sMsg").and_then(Value::as_str).unwrap_or("");
    let client_order_id = optional_string_field(record, "clOrdId");
    if code != "0" {
        return Ok(CancellationOutcome::Rejected {
            code: code.to_owned(),
            message: message.to_owned(),
        });
    }
    Ok(CancellationOutcome::Confirmed {
        order_id: string_field(record, "ordId")?,
        client_order_id,
    })
}

fn string_field(record: &Map<String, Value>, field: &str) -> Result<String, AtkError> {
    record
        .get(field)
        .and_then(Value::as_str)
        .filter(|value| !value.trim().is_empty())
        .map(ToOwned::to_owned)
        .ok_or_else(|| AtkError::ResponseInvalid(format!("{field} is missing")))
}

fn optional_string_field(record: &Map<String, Value>, field: &str) -> String {
    record
        .get(field)
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_owned()
}

fn decimal_field(record: &Map<String, Value>, field: &str) -> Result<Decimal, AtkError> {
    record
        .get(field)
        .ok_or_else(|| AtkError::ResponseInvalid(format!("{field} is missing")))
        .and_then(|value| decimal_value(value, field))
}

fn decimal_value(value: &Value, field: &str) -> Result<Decimal, AtkError> {
    let text = value
        .as_str()
        .map(ToOwned::to_owned)
        .or_else(|| value.as_number().map(ToString::to_string))
        .ok_or_else(|| AtkError::InvalidNumber {
            field: field.to_owned(),
            value: value.to_string(),
        })?;
    Decimal::from_str(&text).map_err(|_| AtkError::InvalidNumber {
        field: field.to_owned(),
        value: text,
    })
}

fn timestamp_field(
    value: Option<&Value>,
    fallback: Option<&Value>,
) -> Result<OffsetDateTime, AtkError> {
    let value = value
        .or(fallback)
        .ok_or_else(|| AtkError::ResponseInvalid("timestamp is missing".to_owned()))?;
    if let Some(text) = value.as_str() {
        if let Ok(milliseconds) = text.parse::<i128>() {
            return OffsetDateTime::from_unix_timestamp_nanos(milliseconds * 1_000_000)
                .map_err(|error| AtkError::ResponseInvalid(error.to_string()));
        }
        return OffsetDateTime::parse(text, &time::format_description::well_known::Rfc3339)
            .map_err(|error| AtkError::ResponseInvalid(error.to_string()));
    }
    if let Some(milliseconds) = value.as_i64() {
        return OffsetDateTime::from_unix_timestamp_nanos(i128::from(milliseconds) * 1_000_000)
            .map_err(|error| AtkError::ResponseInvalid(error.to_string()));
    }
    Err(AtkError::ResponseInvalid(
        "timestamp has an unsupported type".to_owned(),
    ))
}

fn mutation_unknown_reason(error: &AtkError) -> String {
    match error {
        AtkError::Transport(McpError::RequestTimeout { .. }) => "request timeout".to_owned(),
        AtkError::Transport(McpError::ProcessExited) => "process exited".to_owned(),
        AtkError::Transport(McpError::MalformedJson(_)) => "malformed MCP response".to_owned(),
        AtkError::ProtocolResponseInvalid(_) | AtkError::ResponseInvalid(_) => {
            "invalid MCP response".to_owned()
        }
        other => format!("ambiguous mutation: {other}"),
    }
}
