use prooftrade::domain::{
    CounterSignal, DecisionId, EvidenceDirection, EvidenceId, EvidenceReference, Instrument,
    LineageId, MarketType, PositionId, PositionState, RankedOpportunity, SourceEventId, SourceKind,
    TradeAction, TradeDecision, TradingUniverse, rank_opportunities,
};
use rust_decimal::Decimal;
use time::OffsetDateTime;
use uuid::Uuid;

fn timestamp() -> OffsetDateTime {
    OffsetDateTime::from_unix_timestamp(1_700_000_000).unwrap()
}

fn id(value: u128) -> Uuid {
    Uuid::from_u128(value)
}

#[test]
fn initial_universe_contains_the_four_configured_spot_symbols() {
    let universe = TradingUniverse::initial();

    let symbols: Vec<_> = universe
        .instruments()
        .iter()
        .map(Instrument::symbol)
        .collect();

    assert_eq!(symbols, ["BTC-USDT", "ETH-USDT", "SOL-USDT", "HYPE-USDT"]);
}

#[test]
fn ranking_orders_candidates_by_score_and_uses_symbol_for_ties() {
    let candidates = vec![
        prooftrade::domain::OpportunityCandidate::new(
            Instrument::parse("BTC-USDT").unwrap(),
            Decimal::new(71, 2),
        )
        .unwrap(),
        prooftrade::domain::OpportunityCandidate::new(
            Instrument::parse("SOL-USDT").unwrap(),
            Decimal::new(86, 2),
        )
        .unwrap(),
        prooftrade::domain::OpportunityCandidate::new(
            Instrument::parse("ETH-USDT").unwrap(),
            Decimal::new(71, 2),
        )
        .unwrap(),
    ];

    let ranked = rank_opportunities(candidates).unwrap();

    assert_eq!(
        ranked,
        vec![
            RankedOpportunity::new(
                1,
                Instrument::parse("SOL-USDT").unwrap(),
                Decimal::new(86, 2)
            )
            .unwrap(),
            RankedOpportunity::new(
                2,
                Instrument::parse("BTC-USDT").unwrap(),
                Decimal::new(71, 2)
            )
            .unwrap(),
            RankedOpportunity::new(
                3,
                Instrument::parse("ETH-USDT").unwrap(),
                Decimal::new(71, 2)
            )
            .unwrap(),
        ]
    );
}

#[test]
fn counter_signal_requires_counter_direction_and_preserves_lineage() {
    let mut evidence = EvidenceReference::new(
        EvidenceId::from_uuid(id(2)),
        SourceKind::MarxFinance,
        SourceEventId::new("marx-event-1").unwrap(),
        LineageId::from_uuid(id(3)),
        timestamp(),
    )
    .unwrap();
    evidence.direction = EvidenceDirection::Counter;
    evidence.instrument = Some(Instrument::parse("BTC-USDT").unwrap());
    evidence.strength = Decimal::new(65, 2);

    let counter_signal = CounterSignal::new(evidence.clone(), "bearish thesis").unwrap();

    assert_eq!(
        counter_signal.evidence.evidence_id,
        EvidenceId::from_uuid(id(2))
    );
    assert_eq!(
        counter_signal.evidence.lineage_id,
        LineageId::from_uuid(id(3))
    );
    assert_eq!(counter_signal.summary, "bearish thesis");
}

#[test]
fn hold_decision_is_valid_without_exposure_change_and_keeps_reconsideration_context() {
    let mut decision = TradeDecision::hold(
        DecisionId::from_uuid(id(4)),
        Instrument::parse("BTC-USDT").unwrap(),
        timestamp(),
        "catalyst is already priced",
    );
    decision
        .reconsider_if
        .push("volume confirms breakout".to_owned());

    decision.validate().unwrap();

    assert_eq!(decision.action, TradeAction::Hold);
    assert!(decision.requested_notional.is_none());
    assert!(decision.desired_exposure_pct.is_none());
    assert!(decision.desired_reduction_pct.is_none());
}

#[test]
fn sell_decision_is_invalid_for_a_flat_spot_position() {
    let decision = TradeDecision::sell(
        DecisionId::from_uuid(id(5)),
        Instrument::parse("BTC-USDT").unwrap(),
        timestamp(),
        Decimal::new(40, 2),
    );
    let position = PositionState::flat(Instrument::parse("BTC-USDT").unwrap(), timestamp());

    let error = decision.validate_for_position(&position).unwrap_err();

    assert!(matches!(
        error,
        prooftrade::domain::DomainError::SellWithoutInventory
    ));
}

#[test]
fn open_position_rejects_negative_quantity() {
    let error = PositionState::open(
        PositionId::from_uuid(id(6)),
        Instrument::parse("BTC-USDT").unwrap(),
        Decimal::new(-1, 0),
        Decimal::new(100, 0),
        timestamp(),
        timestamp(),
    )
    .unwrap_err();

    assert!(matches!(
        error,
        prooftrade::domain::DomainError::InvalidQuantity
    ));
}

#[test]
fn trade_decision_rejects_strength_above_one() {
    let mut decision = TradeDecision::buy(
        DecisionId::from_uuid(id(7)),
        Instrument::parse("BTC-USDT").unwrap(),
        timestamp(),
        Decimal::new(100, 0),
        Decimal::new(18, 2),
    );
    decision.confidence = Decimal::new(101, 2);

    let error = decision.validate().unwrap_err();

    assert!(matches!(
        error,
        prooftrade::domain::DomainError::OutOfRange { .. }
    ));
}

#[test]
fn buy_decision_rejects_zero_desired_exposure() {
    let decision = TradeDecision::buy(
        DecisionId::from_uuid(id(8)),
        Instrument::parse("BTC-USDT").unwrap(),
        timestamp(),
        Decimal::new(100, 0),
        Decimal::ZERO,
    );

    let error = decision.validate().unwrap_err();

    assert!(matches!(
        error,
        prooftrade::domain::DomainError::NotPositive(_)
    ));
}

#[test]
fn deserialized_universe_can_be_checked_for_duplicate_instruments() {
    let universe: TradingUniverse =
        serde_json::from_str(r#"{"instruments":["BTC-USDT","BTC-USDT"]}"#).unwrap();

    let error = universe.validate().unwrap_err();

    assert!(matches!(
        error,
        prooftrade::domain::DomainError::DuplicateInstrument(_)
    ));
}

#[test]
fn uppercase_numeric_asset_symbol_is_a_valid_spot_instrument() {
    let instrument = Instrument::parse("1000SATS-USDT").unwrap();

    assert_eq!(instrument.symbol(), "1000SATS-USDT");
    assert_eq!(instrument.market_type(), MarketType::Spot);
}

#[test]
fn position_state_validation_rejects_a_deserialized_negative_quantity() {
    let position = PositionState::Open {
        position_id: PositionId::from_uuid(id(9)),
        instrument: Instrument::parse("BTC-USDT").unwrap(),
        quantity: Decimal::new(-1, 0),
        average_entry_price: Decimal::new(100, 0),
        opened_at: timestamp(),
        updated_at: timestamp(),
    };

    let error = position.validate().unwrap_err();

    assert!(matches!(
        error,
        prooftrade::domain::DomainError::InvalidQuantity
    ));
}
