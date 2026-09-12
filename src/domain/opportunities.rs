use std::collections::HashSet;

use rust_decimal::Decimal;
use serde::{Deserialize, Serialize};

use super::{DomainError, Instrument, evidence::validate_unit};

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct OpportunityCandidate {
    pub instrument: Instrument,
    pub opportunity_score: Decimal,
}

impl OpportunityCandidate {
    pub fn new(instrument: Instrument, opportunity_score: Decimal) -> Result<Self, DomainError> {
        validate_unit("opportunity score", opportunity_score)?;
        Ok(Self {
            instrument,
            opportunity_score,
        })
    }
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct RankedOpportunity {
    pub rank: u32,
    pub instrument: Instrument,
    pub opportunity_score: Decimal,
}

impl RankedOpportunity {
    pub fn new(
        rank: u32,
        instrument: Instrument,
        opportunity_score: Decimal,
    ) -> Result<Self, DomainError> {
        if rank == 0 {
            return Err(DomainError::InvalidOpportunityRank);
        }
        validate_unit("opportunity score", opportunity_score)?;
        Ok(Self {
            rank,
            instrument,
            opportunity_score,
        })
    }
}

pub fn rank_opportunities(
    mut candidates: Vec<OpportunityCandidate>,
) -> Result<Vec<RankedOpportunity>, DomainError> {
    let mut instruments = HashSet::with_capacity(candidates.len());
    for candidate in &candidates {
        if !instruments.insert(candidate.instrument.clone()) {
            return Err(DomainError::DuplicateOpportunityInstrument(
                candidate.instrument.to_string(),
            ));
        }
    }

    candidates.sort_by(|left, right| {
        right
            .opportunity_score
            .cmp(&left.opportunity_score)
            .then_with(|| left.instrument.cmp(&right.instrument))
    });

    candidates
        .into_iter()
        .enumerate()
        .map(|(index, candidate)| {
            RankedOpportunity::new(
                (index + 1) as u32,
                candidate.instrument,
                candidate.opportunity_score,
            )
        })
        .collect()
}
