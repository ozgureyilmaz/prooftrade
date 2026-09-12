use std::collections::HashMap;

use crate::domain::{LineageId, SourceEventId};

use super::{EventFingerprint, IntelligenceError, IntelligenceEvidence, IntelligenceSource};

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum IngestDisposition {
    NewCatalyst,
    UpdatedSourceEvent,
    RelatedEvidence,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct IngestOutcome {
    pub evidence_id: crate::domain::EvidenceId,
    pub lineage_id: LineageId,
    pub disposition: IngestDisposition,
}

#[derive(Default)]
pub struct LineageLedger {
    observations: Vec<IntelligenceEvidence>,
    source_events: HashMap<(IntelligenceSource, String), usize>,
    lineages: HashMap<EventFingerprint, LineageId>,
}

impl LineageLedger {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn ingest(
        &mut self,
        mut evidence: IntelligenceEvidence,
    ) -> Result<IngestOutcome, IntelligenceError> {
        evidence.validate()?;
        let source_event_key = (evidence.source, source_event_key(&evidence.source_event_id));

        if let Some(index) = self.source_events.get(&source_event_key).copied() {
            let (evidence_id, lineage_id) = {
                let existing = &self.observations[index];
                (
                    existing.evidence_id,
                    existing.lineage_id.ok_or(IntelligenceError::EmptyField {
                        field: "lineage_id",
                    })?,
                )
            };
            evidence.evidence_id = evidence_id;
            evidence.lineage_id = Some(lineage_id);
            self.observations[index] = evidence;
            return Ok(IngestOutcome {
                evidence_id,
                lineage_id,
                disposition: IngestDisposition::UpdatedSourceEvent,
            });
        }

        let (lineage_id, disposition) = match self.lineages.get(&evidence.event_fingerprint) {
            Some(lineage_id) => (*lineage_id, IngestDisposition::RelatedEvidence),
            None => {
                let lineage_id = LineageId::new();
                self.lineages
                    .insert(evidence.event_fingerprint.clone(), lineage_id);
                (lineage_id, IngestDisposition::NewCatalyst)
            }
        };
        evidence.lineage_id = Some(lineage_id);
        let evidence_id = evidence.evidence_id;
        let index = self.observations.len();
        self.observations.push(evidence);
        self.source_events.insert(source_event_key, index);

        Ok(IngestOutcome {
            evidence_id,
            lineage_id,
            disposition,
        })
    }

    pub fn observations(&self) -> &[IntelligenceEvidence] {
        &self.observations
    }
}

fn source_event_key(source_event_id: &SourceEventId) -> String {
    source_event_id.as_str().to_owned()
}
