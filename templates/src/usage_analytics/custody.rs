//! Historical custody protocol fixtures; production uploads use protocol 2.
use super::events::logical_event_id;
use super::events::*;
use serde::{Deserialize, Serialize};
use std::collections::BTreeSet;
pub const PROTOCOL_VERSION: u8 = 1;
pub const MAX_BATCH_BYTES: usize = 256 * 1024;
pub const MAX_BATCH_EVENTS: usize = 100;

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(try_from = "u8", into = "u8")]
pub struct Version;

impl TryFrom<u8> for Version {
    type Error = ContractError;
    fn try_from(value: u8) -> Result<Self, Self::Error> {
        (value == PROTOCOL_VERSION)
            .then_some(Self)
            .ok_or(ContractError::InvalidRequest)
    }
}

impl From<Version> for u8 {
    fn from(_: Version) -> Self {
        PROTOCOL_VERSION
    }
}

/// Each operation has a closed set of fields. There is no caller account field.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(tag = "command", rename_all = "snake_case", deny_unknown_fields)]
pub enum Request {
    Status {
        protocol_version: Version,
    },
    Enable {
        protocol_version: Version,
        source: Source,
        lifecycle_epoch: LifecycleEpoch,
    },
    Disable {
        protocol_version: Version,
        source: Source,
        provenance: Provenance,
    },
    Select {
        protocol_version: Version,
        source: Source,
        provenance: Provenance,
        through_sequence: Counter,
    },
    Sync {
        protocol_version: Version,
        source: Source,
        provenance: Provenance,
        batch_id: CanonicalUuid,
        ordered_membership_sha256: Sha256Hash,
        events: Vec<EventSubmission>,
    },
    Receipt {
        protocol_version: Version,
        source: Source,
        batch_id: CanonicalUuid,
    },
    ClearPreview {
        protocol_version: Version,
        source: Source,
        lifecycle_epoch: LifecycleEpoch,
    },
    ClearConfirm {
        protocol_version: Version,
        source: Source,
        lifecycle_epoch: LifecycleEpoch,
        preview_id: CanonicalUuid,
        preview_sha256: Sha256Hash,
    },
    History {
        protocol_version: Version,
        source: Source,
        after_sequence: Option<Counter>,
        through_sequence: Option<SequenceBoundary>,
        limit: u16,
    },
}

impl Request {
    /// Parse the exact received bytes before decoding JSON or allocating records.
    pub fn parse(text: &str) -> Result<Self, ContractError> {
        if text.len() >= MAX_BATCH_BYTES {
            return Err(ContractError::BoundsExceeded);
        }
        let value = parse_exact_json(text).map_err(|_| ContractError::InvalidRequest)?;
        let request: Self =
            serde_json::from_value(value).map_err(|_| ContractError::InvalidRequest)?;
        match &request {
            Self::Sync { events, .. } if events.is_empty() || events.len() > MAX_BATCH_EVENTS => {
                return Err(ContractError::BoundsExceeded);
            }
            Self::History { limit, .. }
                if *limit == 0 || usize::from(*limit) > MAX_BATCH_EVENTS =>
            {
                return Err(ContractError::BoundsExceeded);
            }
            Self::History {
                after_sequence: Some(after),
                through_sequence,
                ..
            } if through_sequence.is_none_or(|through| through.get() < after.get()) => {
                return Err(ContractError::InvalidRequest);
            }
            _ => {}
        }
        Ok(request)
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Custody {
    Buffered,
    Retained,
    AnalyticsReady,
}

/// The admission manifest commitment is opaque to clients. Persist it from the
/// authenticated admission response alongside the immutable selected batch; never
/// reconstruct server identity or replace it from a later receipt response.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ReceiptExpectation {
    pub source: Source,
    pub provenance: Provenance,
    pub batch_id: CanonicalUuid,
    pub manifest_sha256: Sha256Hash,
    pub ordered_membership_sha256: Sha256Hash,
    pub members: Vec<ReceiptMember>,
}

/// Authenticated admission binds an opaque server manifest commitment to the
/// client's immutable batch. It is separate from buffered or retained custody.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct AdmissionCommitment {
    pub protocol_version: Version,
    pub source: Source,
    pub provenance: Provenance,
    pub batch_id: CanonicalUuid,
    pub ordered_membership_sha256: Sha256Hash,
    pub manifest_sha256: Sha256Hash,
}

impl AdmissionCommitment {
    pub fn parse(text: &str) -> Result<Self, ContractError> {
        if text.len() >= MAX_BATCH_BYTES {
            return Err(ContractError::BoundsExceeded);
        }
        let value = parse_exact_json(text).map_err(|_| ContractError::InvalidRequest)?;
        serde_json::from_value(value).map_err(|_| ContractError::InvalidRequest)
    }
    pub fn verify(
        &self,
        source: &Source,
        provenance: &Provenance,
        batch_id: &CanonicalUuid,
        members: &[ReceiptMember],
    ) -> Result<(), ContractError> {
        if &self.source != source || &self.provenance != provenance || &self.batch_id != batch_id {
            return Err(ContractError::ScopeMismatch);
        }
        if ordered_membership_sha256(source, members)? != self.ordered_membership_sha256 {
            return Err(ContractError::HashMismatch);
        }
        Ok(())
    }
}

/// Client-computable membership excludes tenant identity. The server separately
/// binds it to authenticated account identity in its opaque admission manifest.
pub fn ordered_membership_sha256(
    source: &Source,
    members: &[ReceiptMember],
) -> Result<Sha256Hash, ContractError> {
    if members.is_empty() || members.len() > MAX_BATCH_EVENTS {
        return Err(ContractError::BoundsExceeded);
    }
    #[derive(Serialize)]
    struct Entry<'a> {
        source: &'a Source,
        event_id: &'a str,
        semantic_hash_version: SemanticHashVersion,
        semantic_sha256: &'a Sha256Hash,
        original_sha256: &'a Sha256Hash,
        #[serde(skip_serializing_if = "Option::is_none")]
        timing_sha256: Option<&'a Sha256Hash>,
    }
    let mut ids = BTreeSet::new();
    let mut entries = Vec::with_capacity(members.len());
    for member in members {
        if !logical_event_id(&member.event_id) {
            return Err(ContractError::InvalidRecord);
        }
        if !ids.insert(&member.event_id) {
            return Err(ContractError::DuplicateEvent);
        }
        entries.push(Entry {
            source,
            event_id: &member.event_id,
            semantic_hash_version: member.semantic_hash_version,
            semantic_sha256: &member.semantic_sha256,
            original_sha256: &member.original_sha256,
            timing_sha256: member.timing_sha256.as_ref(),
        });
    }
    canonical_hash("cargo-ai-usage-membership-v1", &entries)
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Receipt {
    pub protocol_version: Version,
    pub source: Source,
    pub provenance: Provenance,
    pub batch_id: CanonicalUuid,
    pub manifest_sha256: Sha256Hash,
    pub ordered_membership_sha256: Sha256Hash,
    pub custody: Custody,
    pub members: Vec<ReceiptMember>,
}

impl Receipt {
    pub fn parse(text: &str) -> Result<Self, ContractError> {
        if text.len() >= MAX_BATCH_BYTES {
            return Err(ContractError::BoundsExceeded);
        }
        let value = parse_exact_json(text).map_err(|_| ContractError::InvalidRequest)?;
        let receipt: Self =
            serde_json::from_value(value).map_err(|_| ContractError::InvalidRequest)?;
        if receipt.members.is_empty() || receipt.members.len() > MAX_BATCH_EVENTS {
            return Err(ContractError::BoundsExceeded);
        }
        Ok(receipt)
    }

    /// Retention permits local custody reconciliation independently of analytics
    /// readiness. Buffered acknowledgments never satisfy this check.
    pub fn verify_retained(&self, expected: &ReceiptExpectation) -> Result<(), ContractError> {
        if self.custody == Custody::Buffered {
            return Err(ContractError::NotRetained);
        }
        if ordered_membership_sha256(&expected.source, &expected.members)?
            != expected.ordered_membership_sha256
        {
            return Err(ContractError::HashMismatch);
        }
        if self.source != expected.source
            || self.provenance != expected.provenance
            || self.batch_id != expected.batch_id
            || self.manifest_sha256 != expected.manifest_sha256
            || self.ordered_membership_sha256 != expected.ordered_membership_sha256
            || self.members != expected.members
        {
            return Err(ContractError::ReceiptMismatch);
        }
        Ok(())
    }
}
