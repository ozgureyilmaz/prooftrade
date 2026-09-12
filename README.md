# ProofTrade

ProofTrade is an evidence-driven, spot-only trading core for the Komünite
Agentic Trading Hackathon with OKX TR.

Current checkout status: the Polymarket integration has a read-only hybrid
MCP + Gamma/CLOB REST adapter with runtime disable/enable controls. The local
deterministic build gate passes; live external availability remains an
operator check.

## Build and verify

```bash
cargo fmt --check
cargo check --workspace
cargo clippy --workspace --all-targets --all-features -- -D warnings
cargo test --workspace
```

The default path is deterministic simulation. The explicit 30-USDT scenario
uses `SimulationConfig::new(Decimal::from(30))` and never reaches ATK.

## Runtime boundaries

`prooftrade-agent` investigates and emits strict proposed decisions. The
`RiskEngine` owns capital permission; `ExecutionEngine` owns order planning,
client identity, UNKNOWN state, and reconciliation. Official OKX ATK MCP is
the operational OKX boundary. SQLite/WAL audit storage is runtime-owned and
secrets are redacted.

The canonical agent artifacts are
[`prompts/prooftrade-agent.md`](prompts/prooftrade-agent.md) and
[`docs/prooftrade-agent-contract.md`](docs/prooftrade-agent-contract.md).
The operator read model is in `src/operator.rs`; the static dashboard is
[`ui/index.html`](ui/index.html). A host should serve the HTML and route its
read/control calls to `OperatorController`; there is intentionally no raw order
endpoint.

## Reddit read-only intelligence

ProofTrade reuses the external TypeScript detector at
`/Users/0x79de/Documents/ChatGPT/reddit-growth-engine` through the typed
`RedditCryptoReader`/`RedditAdapter` process boundary. Before any live-read
command, export the private Reddit environment in the same shell:

```bash
set -a
source /Users/0x79de/.config/reddit-hermes/.env
set +a
unset REDDIT_ACCESS_TOKEN
```

No autonomous publishing grant is needed for this path. The detector must
remain `LIVE_READ_ONLY`, `dryRun: true`, `publisherState: DISABLED`, and
`actions: []`. Reddit evidence is persisted by ProofTrade runtime storage after
normalization; the external detector database is only its bounded checkpoint
state. Reddit can trigger targeted research and provide supporting/counter
context, but it cannot bypass RiskEngine or authorize an order.

See [`docs/acceptance.md`](docs/acceptance.md) and
[`docs/okx-integration.md`](docs/okx-integration.md) for validation and safe
ATK smoke-test boundaries. The optional read-only Polymarket MCP + Gamma/CLOB
REST connection and runtime disable/enable controls are documented in
[`docs/polymarket-integration.md`](docs/polymarket-integration.md). The local
operating commands are in [`docs/runbook.md`](docs/runbook.md).
