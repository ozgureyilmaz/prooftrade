use std::collections::{BTreeMap, BTreeSet};

use crate::domain::SourceEventId;

use super::sources::{IntelligenceSourceAdapter, SourceError};
use super::{
    BoundedEvidenceQuery, IntelligenceError, IntelligenceEvidence, IntelligenceSource,
    LineageLedger, SourceHealthRegistry,
};
use crate::intelligence::sources::polymarket::{
    PolymarketAdapter, PolymarketConfig, PolymarketHybridReader, PolymarketMcpError,
    PolymarketMcpReader, PolymarketRestReader,
};
use crate::intelligence::sources::reddit::{RedditAdapter, RedditCryptoConfig, RedditCryptoReader};

#[derive(Clone, Debug)]
pub struct IntelligenceSnapshot {
    as_of: time::OffsetDateTime,
    observations: Vec<IntelligenceEvidence>,
    source_health: SourceHealthRegistry,
    source_errors: Vec<SourceError>,
    disabled_sources: BTreeSet<IntelligenceSource>,
    truncated: bool,
}

impl IntelligenceSnapshot {
    pub fn as_of(&self) -> time::OffsetDateTime {
        self.as_of
    }

    pub fn observations(&self) -> &[IntelligenceEvidence] {
        &self.observations
    }

    pub fn source_health(&self) -> &SourceHealthRegistry {
        &self.source_health
    }

    pub fn source_errors(&self) -> &[SourceError] {
        &self.source_errors
    }

    pub fn disabled_sources(&self) -> &BTreeSet<IntelligenceSource> {
        &self.disabled_sources
    }

    pub fn was_truncated(&self) -> bool {
        self.truncated
    }
}

pub struct IntelligenceService {
    adapters: Vec<Box<dyn IntelligenceSourceAdapter>>,
    ledger: LineageLedger,
    health: SourceHealthRegistry,
}

impl IntelligenceService {
    pub fn new() -> Self {
        Self {
            adapters: Vec::new(),
            ledger: LineageLedger::new(),
            health: SourceHealthRegistry::new(),
        }
    }

    pub fn register<A>(&mut self, adapter: A)
    where
        A: IntelligenceSourceAdapter + 'static,
    {
        self.adapters.push(Box::new(adapter));
    }

    /// Register the external TypeScript detector through its read-only Rust
    /// process boundary. The detector is never an execution or publishing
    /// adapter.
    pub fn register_reddit_crypto(&mut self, config: RedditCryptoConfig) {
        self.register(RedditAdapter::new(RedditCryptoReader::from_config(config)));
    }

    /// Register the read-only Polymarket MCP source. It remains toggleable
    /// after registration without rebuilding the service.
    pub fn register_polymarket_mcp(
        &mut self,
        config: PolymarketConfig,
    ) -> Result<(), PolymarketMcpError> {
        let enabled = config.enabled();
        let reader = PolymarketMcpReader::launch(config)?;
        self.register(PolymarketAdapter::with_enabled(reader, enabled));
        Ok(())
    }

    /// Register both read-only Polymarket paths. MCP provides semantic
    /// discovery while REST provides deterministic live midpoint/spread data.
    pub fn register_polymarket_hybrid(
        &mut self,
        config: PolymarketConfig,
    ) -> Result<(), PolymarketMcpError> {
        let enabled = config.enabled();
        let mcp = PolymarketMcpReader::launch(config.clone());
        let rest = PolymarketRestReader::launch(config);
        let reader = match (mcp, rest) {
            (Ok(mcp), Ok(rest)) => PolymarketHybridReader::new(Some(mcp), Some(rest))?,
            (Ok(mcp), Err(_rest_error)) => PolymarketHybridReader::new(Some(mcp), None)?,
            (Err(_mcp_error), Ok(rest)) => PolymarketHybridReader::new(None, Some(rest))?,
            (Err(mcp_error), Err(_rest_error)) => return Err(mcp_error),
        };
        self.register(PolymarketAdapter::with_enabled(reader, enabled));
        Ok(())
    }

    pub fn register_polymarket_rest(
        &mut self,
        config: PolymarketConfig,
    ) -> Result<(), PolymarketMcpError> {
        let enabled = config.enabled();
        let reader = PolymarketRestReader::launch(config)?;
        self.register(PolymarketAdapter::with_enabled(reader, enabled));
        Ok(())
    }

    pub fn set_source_enabled(&mut self, source: IntelligenceSource, enabled: bool) -> bool {
        let mut found = false;
        for adapter in &mut self.adapters {
            if adapter.source() == source {
                adapter.set_enabled(enabled);
                found = true;
            }
        }
        found
    }

    pub fn is_source_enabled(&self, source: IntelligenceSource) -> bool {
        self.adapters
            .iter()
            .find(|adapter| adapter.source() == source)
            .is_some_and(|adapter| adapter.is_enabled())
    }

    pub fn enable_polymarket(&mut self) -> bool {
        self.set_source_enabled(IntelligenceSource::Polymarket, true)
    }

    pub fn disable_polymarket(&mut self) -> bool {
        self.set_source_enabled(IntelligenceSource::Polymarket, false)
    }

    pub fn collect(
        &mut self,
        query: &BoundedEvidenceQuery,
    ) -> Result<IntelligenceSnapshot, IntelligenceError> {
        self.collect_internal(query, None)
    }

    pub(crate) fn collect_source(
        &mut self,
        source: IntelligenceSource,
        query: &BoundedEvidenceQuery,
    ) -> Result<IntelligenceSnapshot, IntelligenceError> {
        self.collect_internal(query, Some(source))
    }

    fn collect_internal(
        &mut self,
        query: &BoundedEvidenceQuery,
        only_source: Option<IntelligenceSource>,
    ) -> Result<IntelligenceSnapshot, IntelligenceError> {
        let mut source_errors = Vec::new();
        let mut disabled_sources = BTreeSet::new();
        for adapter in &mut self.adapters {
            let source = adapter.source();
            if only_source.is_some_and(|requested| requested != source) {
                continue;
            }
            if !adapter.is_enabled() {
                self.health.record_disabled(source, query.as_of());
                disabled_sources.insert(source);
                continue;
            }
            match adapter.collect(query) {
                Ok(observations) => {
                    let mut invalid_observation = false;
                    for observation in observations {
                        if query.matches(&observation) {
                            if let Err(error) = self.ledger.ingest(observation) {
                                invalid_observation = true;
                                source_errors.push(SourceError::new(
                                    source,
                                    super::SourceErrorCategory::InvalidData,
                                ));
                                let _ = error;
                            }
                        }
                    }
                    if invalid_observation {
                        self.health.record_failure(
                            source,
                            query.as_of(),
                            super::SourceErrorCategory::InvalidData,
                        );
                    } else {
                        self.health.record_success(source, query.as_of());
                    }
                }
                Err(error) => {
                    self.health
                        .record_failure(source, query.as_of(), error.category());
                    source_errors.push(error);
                }
            }
        }

        let mut observations = self
            .ledger
            .observations()
            .iter()
            .filter(|observation| {
                only_source.is_none_or(|source| source == observation.source)
                    && !disabled_sources.contains(&observation.source)
                    && query.matches(observation)
            })
            .cloned()
            .collect::<Vec<_>>();
        observations.sort_by(|left, right| {
            right
                .observed_at
                .cmp(&left.observed_at)
                .then_with(|| left.source.cmp(&right.source))
                .then_with(|| {
                    source_event_key(&left.source_event_id)
                        .cmp(source_event_key(&right.source_event_id))
                })
        });

        let mut per_source = BTreeMap::<IntelligenceSource, usize>::new();
        let mut bounded = Vec::new();
        let mut truncated = false;
        for observation in observations {
            let source_count = per_source.entry(observation.source).or_default();
            if *source_count >= query.max_per_source() || bounded.len() >= query.max_results() {
                truncated = true;
                continue;
            }
            *source_count += 1;
            bounded.push(observation);
        }

        Ok(IntelligenceSnapshot {
            as_of: query.as_of(),
            observations: bounded,
            source_health: self.health.clone(),
            source_errors,
            disabled_sources,
            truncated,
        })
    }
}

impl Default for IntelligenceService {
    fn default() -> Self {
        Self::new()
    }
}

fn source_event_key(source_event_id: &SourceEventId) -> &str {
    source_event_id.as_str()
}
