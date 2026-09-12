# ProofTrade Engineering Contract

## Project

ProofTrade is an autonomous, evidence-driven, event-based spot-trading system
for the Komünite Agentic Trading Hackathon with OKX TR. The current priority is
a small, reliable, explainable trading core.

## Canonical context

Before substantial implementation, every agent must read:

1. `PLAN_PROMPT.md` — canonical architecture, MUST requirements, and the
   twelve-section build order.
2. `CHAT_CONTEXT.md` — product rationale, accepted decisions, history, and
   intentionally deferred items.
3. `AGENTS.md` — this persistent repository engineering contract.

The latest explicit owner instruction wins; a section-specific command controls
what may be built in that session.

## Hackathon priorities

```text
Functional Utility & Value         30%
User Experience & Interaction      30%
ATK MCP Integration Depth          20%
System Reliability & Safety        10%
Innovation & Uniqueness            10%
```

## Non-negotiable MUST requirements

- `MUST-001`: design for the hackathon’s useful, explainable, safe outcome.
- `MUST-002`: the official OKX Agent Trade Kit MCP is the operational boundary.
  CLI is diagnostic/fallback tooling and must never become a second order
  writer.
- `MUST-003`: Marx Finance is a distinct ProofTrade Event Intelligence source.
- `MUST-004`: reuse relevant read-only intelligence capabilities from
  `ozgureyilmaz/reddit-growth-engine`; do not rebuild its intelligence layer
  without authorization.

## Core architecture boundary

```text
External Intelligence
        ↓
Hermes / prooftrade-agent
        ↓
TradeDecision
        ↓
RiskEngine
        ↓
ExecutionEngine
        ↓
Official OKX ATK MCP
        ↓
OKX
```

Hermes owns investigation and reasoning. RiskEngine owns capital permission.
ExecutionEngine owns execution mechanics. Official ATK MCP owns the OKX
operational interface. External intelligence informs decisions but does not
place orders directly.

## Technology

- ProofTrade core: Rust; async runtime when needed: Tokio.
- Serialization/schema: `serde`, `serde_json`, and later `schemars` where
  useful; financial arithmetic: `rust_decimal`.
- Runtime persistence: `rusqlite` with bundled SQLite, versioned migrations,
  WAL, exact Decimal-as-text payload storage, and runtime-owned writes. The
  crate remains synchronous; `sqlx`/Tokio are not introduced solely for local
  audit storage.
- HTTP/WebSocket: `reqwest` and `tokio-tungstenite` when their sections are
  authorized; logging: `tracing`.
- ATK runtime: official Node/TypeScript implementation, preferably Node.js 22
  LTS, isolated behind MCP stdio.
- Reddit: reuse the existing TypeScript `reddit-growth-engine` read-only
  capability.
- Polymarket: `ozgureyilmaz/polymarket-mcp` for agent-facing discovery plus
  deterministic REST; add WebSocket only when justified.
- Hermes: separate profile/runtime named `prooftrade-agent`.

## Trading constraints

- V1 is spot-only: no leverage, futures, perpetuals, or synthetic shorts.
- Initial universe: `BTC-USDT`, `ETH-USDT`, `SOL-USDT`, `HYPE-USDT`.
- Actions are `BUY`, `SELL`, and `HOLD`; SELL reduces/closes spot inventory.
- Maximum concurrent positions is initially 3 and remains configurable.

## Evidence philosophy

Supporting evidence and counter-evidence are separate concepts; preserve
`support_strength`, `counter_signal_strength`, confidence, stable IDs,
timestamps, source references, and event lineage. HOLD is a successful,
first-class no-exposure-change decision. Absence of sufficient proof may justify
HOLD. Repeated manifestations of one underlying event must not automatically
count as independent evidence.

## Execution safety invariants

- An ATK timeout is not proof that an order failed.
- UNKNOWN execution state must be reconciled before retry; blind order retry is
  forbidden.
- Duplicate exposure prevention is a hard invariant.
- Restart requires reconciliation before adding new risk.
- A typed `RiskDecision` is mandatory before an `ExecutionPlan`; live policy
  may return constrained approval, while simulation/demo preserve valid agent
  sizing and reject unaffordable requests.
- Client-order identity is created before placement and remains stable across
  reconciliation; simulation uses a separate executor and never calls ATK.
- The current hard `-15%` stop-loss and `+30%` take-profit defaults mean
  automatic full exit; later sections own their implementation.
- Secrets, credentials, live trading, and production writes remain explicitly
  approval-gated.

## Build order

1. Core Domain Contracts
2. Hermes Agent Integration Skeleton
3. Official OKX ATK MCP Integration
4. SimulationEngine
5. Event Intelligence Adapters
6. Decision Orchestration
7. RiskEngine
8. Execution Lifecycle & Reconciliation
9. Persistence / Audit Database
10. Testing & Acceptance Suite
11. Final Hermes System Prompt & Tool Contract
12. UI / Operator Experience

Do not silently continue to the next section.

## Simulation contract now in force

The synthetic execution boundary lives in `src/simulation.rs` and consumes
canonical `Instrument` values plus injected `MarketSnapshot` values. It is
independent of Hermes transport, ATK/MCP, network calls, RiskEngine policy,
event intelligence, and persistence.

- `SimulationConfig::new` accepts arbitrary Decimal synthetic cash; the
  explicit 30-USDT scenario is supported, while the convenience default is
  1,000 USDT.
- Market BUY uses ask and market SELL uses bid. `FixedBps` slippage is adverse;
  configured fees affect cash, cost basis, realized PnL, and reported fills.
- LIMIT orders remain pending until the executable market side reaches/crosses
  the limit. Partial fills keep an explicit remainder and are never silently
  resized or auto-completed by the simulator.
- Weighted entry cost includes entry fees; portfolio equity marks held
  inventory at the conservative bid.
- Configured hard stop-loss/take-profit thresholds default to 15%/30% and
  create full protective exits at the current simulated bid with explicit
  `HardStopLoss`/`HardTakeProfit` reasons.

## Section 2 contract now in force

Hermes remains an independent runtime. `hermes::decode_agent_decision` accepts
only a strict, versioned structured payload, validates provenance and semantics,
and converts it into a canonical Section 1 `TradeDecision` wrapped as a
`ValidatedAgentDecision`. Rejected payloads are explicitly
`INVALID_AGENT_DECISION` / `NO_TRADE`. This wrapper is proposed intent, not
RiskEngine approval, an order, or an exchange operation. No free-form prose can
authorize trading. No final Hermes prompt or tool surface is defined here;
those belong to Section 11.

## Event Intelligence boundary now in force

Section 5 uses durable `intelligence` modules for source-specific evidence,
event fingerprints, lineage/deduplication, freshness, source health, bounded
queries, and snapshots. News, Polymarket, Reddit, and Marx semantics remain
separate; this layer never emits a final BUY/SELL/HOLD decision.

Exact `(source, source_event_id)` repeats update one evidence identity. Distinct
cross-source observations with the same deterministic fingerprint share a
lineage but remain separate evidence. Weak matches stay separate rather than
being merged merely because an asset matches.

`SourceHealthRegistry` distinguishes `Unknown`, `Healthy`, `Degraded`,
`Unavailable`, and intentional `Disabled`. Timeouts are not empty results,
successful empty results remain healthy, and freshness uses event time plus an
injected evaluation time.
Polymarket history is bounded and calculates Decimal probability-point deltas.

ATK News capability is read from the existing `CapabilityReport.news` flag;
missing News returns an explicit capability failure and no transport is
fabricated. Reddit, Marx, and Polymarket use narrow read-only reader seams.
The local Reddit TypeScript project remains external and publishing/growth
capabilities are excluded. Marx live transport remains owned by the separate
Hermes-side integration. Polymarket can use the local `polymarket-mcp` MCP
stdio executable plus direct public Gamma/CLOB REST reads through the optional
hybrid adapter in `src/intelligence/sources/polymarket.rs`; both are read-only
and controlled by the same runtime toggle.

## Decision orchestration boundary now in force

The coordination boundary lives in `src/decision.rs`. It consumes the
canonical universe, intelligence evidence, source health, position state, and
neutral market context, then sends a structured `AgentDecisionRequest` through
injected research and decision-provider seams.

Opportunity ranking is deterministic attention prioritization, not capital
approval. Evidence is scoped to its affected instrument, repeated event
fingerprints are not counted as independent catalysts, and supporting,
counter, and other evidence remain separate for Hermes.

`ResearchSourcePlanner`, `ResearchProvider`, and `ResearchBudget` keep source
selection targeted and bounded. A query must explicitly target its instrument;
source failures remain visible in health/error context and are not converted to
neutral evidence.

`DecisionProvider` output is validated by the existing strict Hermes decoder.
Malformed or failed agent output becomes `NoAction`. `DecisionProposal`
classifies BUY as `Entry` or `Add`, SELL as `Reduction`, and HOLD as a
successful first-class result. Prior evidence IDs suppress unchanged duplicate
actions while new reactions remain available.

Decision orchestration never calls ATK, SimulationEngine, RiskEngine, or an
execution writer; every accepted result remains proposed intent only.

## Risk, execution, and runtime contracts now in force

`src/risk.rs` owns deterministic capital permission. `RiskEngine` validates the
canonical `TradeDecision`, freshness, spot inventory, maximum three open
positions, available cash, safety state, unresolved duplicate exposure, and
ADD evidence qualification. It emits a typed `RiskDecision` with rule names,
reason codes, timestamp, and optional `ApprovedIntent`. Protective full SELLs
can proceed through safe/killed states; live constraints are explicit rather
than silently applied to simulation/demo.

`src/execution.rs` owns `ExecutionPolicy`, `ExecutionPlan`, market/limit
selection, bounded limit lifetime, `ExecutionEngine`, typed lifecycle states,
client-order identity, checkpoint restore, and reconciliation. Unknown
placement/cancellation is never retried blindly. `AtkClient<T>` is adapted
through the existing official MCP seam; `SimulationExecutor` stays isolated.
`src/runtime.rs` composes proposed decisions through RiskEngine and execution
without exposing an order-writer shortcut.

## Persistence, reporting, and operator contracts now in force

`src/persistence.rs` provides runtime-owned SQLite/WAL migration, append-style
audit events, decision/risk/evidence/execution persistence, replay by entity,
rollback-safe decision bundles, restart IDs, reporting counts, and secret-key
redaction. `src/reporting.rs` calculates exact Decimal simulation metrics,
including equity drawdown, PnL, fees, slippage, trades, HOLDs, rejections, and
protective exits.

`src/agent_contract.rs` and `prompts/prooftrade-agent.md` define the separate
`prooftrade-agent` profile and versioned read/research plus proposed-intent
tool surface. `docs/prooftrade-agent-contract.md` is the reviewable contract;
Rust's strict Hermes decoder remains authoritative. No direct exchange order,
risk override, credential, or direct database capability is exposed to Hermes.

`src/operator.rs` provides an operator read model, HTML rendering, and a
narrow confirmed `KILL` control connected to runtime safety state. `ui/index.html`
is a lightweight dashboard client. The current repository exposes route
handling primitives rather than binding a production HTTP listener; hosting
and deployment remain explicit operator work.

## Deferred items

Do not guess or finalize these before their authorized sections:

- exact permanent live exposure and session-loss limits;
- live source transports not yet available to this checkout and the
  owner-managed Hermes Marx integration;
- production HTTP hosting, authentication, and deployment configuration;
- real live mutation smoke tests and operator credentials.

There is no project requirement for a “2-hour validation loop”. Do not add one
to code, tests, prompts, or acceptance criteria unless the owner explicitly
requests it later.

## ATK integration contract now in force

- Official package: `@okx_ai/okx-trade-mcp` from `https://github.com/okx/agent-trade-kit`.
- Verified server version: `1.4.6`; launch through the official `okx-trade-mcp` executable and MCP stdio, not direct OKX REST from Rust.
- The Rust boundary lives under `src/atk/` and discovers `system_get_capabilities`, `market_get_ticker`, `account_get_balance`, `spot_place_order`, `spot_get_order`, and `spot_cancel_order` at startup.
- Default ATK config is `demo + read_only` for OKX TR; the adapter validates server version, protocol, mode, module availability, tool schemas, and a reported site when available.
- Placement and cancellation timeouts or ambiguous responses become explicit `Unknown` outcomes; the adapter has no blind mutation retry path.
- The official capability snapshot currently reports the selected mode and module availability under `data.capabilities`; site may be unreported and is surfaced in health metadata.
- Credential-free deterministic tests use fake transports and a shell-backed MCP child. The opt-in official smoke test is read-only and must be enabled with `PROOFTRADE_ATK_SMOKE=1`.

## Current progress

- Section 1 — COMPLETE and verified by the current Section 1 test suite.
- Section 2 — COMPLETE and verified by strict adapter tests.
- Section 3 — COMPLETE and verified by deterministic ATK/MCP tests plus the
  opt-in official read-only handshake/market smoke test.
- Section 4 — COMPLETE and verified by the simulator test suite.
- Section 5 — IN PROGRESS: deterministic Event Intelligence contracts and
  adapter seams are implemented; Polymarket has an optional read-only hybrid
  MCP stdio plus Gamma/CLOB REST transport with runtime disable/enable
  controls, Reddit has a bounded typed `LIVE_READ_ONLY` process adapter, while
  Marx and ATK News live transports remain explicitly external.
- Section 6 — COMPLETE and verified by deterministic decision-orchestration
  tests.
- Risk — COMPLETE as a deterministic library boundary, verified by
  `tests/risk_engine.rs`.
- Execution / reconciliation — COMPLETE as a fake-gateway and ATK-adapter
  boundary, verified by `tests/execution_lifecycle.rs` and existing ATK tests.
- Persistence / audit / replay — COMPLETE for the local SQLite contract,
  verified by `tests/persistence.rs`; migration uses `rusqlite`, not `sqlx`.
- Acceptance / reporting — COMPLETE for deterministic and failure-oriented
  local coverage, verified by `tests/acceptance.rs`, `tests/reporting.rs`, and
  the matrix in `docs/acceptance.md`.
- Hermes contract — COMPLETE as a canonical reviewable prompt/tool artifact,
  verified by `tests/agent_contract.rs`.
- Operator experience — COMPLETE as a persisted read-model/router primitive,
  static dashboard, and loopback host, verified by `tests/operator.rs`; public
  authenticated hosting remains open.

Do not mark a section complete without running its relevant validation and
reporting known limitations. Preserve user changes and stop after the
authorized section.

## P0/P1 remediation status

The deterministic remediation pass is currently green under:

```text
cargo fmt --check
cargo check --workspace
cargo clippy --workspace --all-targets --all-features -- -D warnings
cargo test --workspace
```

The pass added/enforced backend mode matching, a hard reconciliation gate,
approved-intent execution-plan binding, shared operator KILL state,
`PROOFTRADE_LIVE_ARM=1` live mutation arming/auth validation, acknowledged and
partial restart reconciliation, protective pending-order cancellation, complete
agent schema properties and request provenance binding, conservative ADD
qualification, atomic lifecycle audit writes, secret-key variant redaction,
Polymarket MCP/REST/hybrid fixtures, and the bounded Reddit `detect-crypto`
read-only adapter.

Remaining external blockers are credentialed official ATK smoke/read-back,
owner-managed Marx and ATK News live transports, authenticated long-running
operator hosting, and recorded presentation video. The standalone local
application runner and bounded MCP tool surface are now implemented. No live
mutation is authorized by this status.
