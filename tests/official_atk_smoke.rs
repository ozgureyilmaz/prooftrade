use std::env;
use std::path::PathBuf;

use prooftrade::atk::{AtkClient, AtkConfig, AtkHealth, AtkSite, TradingMode};
use prooftrade::domain::Instrument;

#[test]
#[ignore = "requires explicit local official ATK configuration; read-only only"]
fn official_atk_read_only_handshake_is_available_when_opted_in() {
    assert_eq!(
        env::var("PROOFTRADE_ATK_SMOKE").ok().as_deref(),
        Some("1"),
        "set PROOFTRADE_ATK_SMOKE=1 to run the credentialed read-only smoke test"
    );
    let executable = env::var_os("PROOFTRADE_ATK_MCP_BIN")
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from("okx-trade-mcp"));
    let config = AtkConfig::new(executable)
        .with_site(AtkSite::Tr)
        .with_mode(TradingMode::Demo)
        .with_read_only(true)
        .with_modules(["market", "spot", "account"]);

    let mut client = AtkClient::launch(config).expect("official ATK read-only handshake");
    assert_eq!(client.health().status, AtkHealth::Ready);
    assert_eq!(client.capabilities().server_version, "1.4.6");
    eprintln!(
        "official ATK read-only smoke: version={} site=TR mode=DEMO read_only={} market={} account={} spot_place={} spot_query={} spot_cancel={} news={} has_auth={} site_verification={:?}",
        client.capabilities().server_version,
        client.capabilities().read_only,
        client.capabilities().market_data,
        client.capabilities().account_balance,
        client.capabilities().spot_order_placement,
        client.capabilities().spot_order_query,
        client.capabilities().spot_order_cancellation,
        client.capabilities().news,
        client.capabilities().has_auth,
        client.capabilities().site_verification,
    );
    let ticker = client
        .market_ticker(&Instrument::parse("BTC-USDT").unwrap())
        .expect("official ATK market read");
    assert!(ticker.last > rust_decimal::Decimal::ZERO);
    client.shutdown().expect("shut down official ATK child");
}

#[test]
#[ignore = "requires explicit local official ATK live read-only configuration"]
fn official_atk_live_read_only_market_and_capabilities_are_available_when_opted_in() {
    let mut client =
        AtkClient::launch(live_read_only_config()).expect("official ATK live read-only handshake");
    assert_eq!(client.health().status, AtkHealth::Ready);
    assert_eq!(client.capabilities().server_version, "1.4.6");
    assert!(!client.capabilities().demo);
    assert!(client.capabilities().read_only);
    assert!(!client.capabilities().spot_order_placement);
    assert!(!client.capabilities().spot_order_cancellation);

    let ticker = client
        .market_ticker(&Instrument::parse("BTC-USDT").unwrap())
        .expect("official ATK live market read");
    assert!(ticker.last > rust_decimal::Decimal::ZERO);
    eprintln!("live read-only market smoke: BTC-USDT last={}", ticker.last);
    client.shutdown().expect("shut down official ATK child");
}

#[test]
#[ignore = "requires explicit local official ATK live read-only configuration"]
fn official_atk_live_read_only_account_is_available_when_opted_in() {
    let mut client =
        AtkClient::launch(live_read_only_config()).expect("official ATK live read-only handshake");
    let balance = client
        .account_balance(None)
        .expect("official ATK live account read");
    eprintln!(
        "live read-only account smoke: balances={}",
        balance.balances.len()
    );
    client.shutdown().expect("shut down official ATK child");
}

fn live_read_only_config() -> AtkConfig {
    assert_eq!(
        env::var("PROOFTRADE_ATK_LIVE_READ_ONLY").ok().as_deref(),
        Some("1"),
        "set PROOFTRADE_ATK_LIVE_READ_ONLY=1 to run the live read-only smoke test"
    );
    let executable = env::var_os("PROOFTRADE_ATK_MCP_BIN")
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from("okx-trade-mcp"));
    let mut config = AtkConfig::new(executable)
        .with_site(AtkSite::Tr)
        .with_mode(TradingMode::Live)
        .with_read_only(true)
        .with_modules(["market", "spot", "account"]);
    if let Some(profile) = env::var_os("PROOFTRADE_ATK_PROFILE") {
        config = config.with_profile(profile.to_string_lossy());
    }
    config
}
