//! Versioned analytics wire values, canonical hashes and exact custody checks.
//! Payload privacy is enforced by the existing closed projection before construction.

use serde::{de, Deserialize, Serialize};
use serde_json::Value;
use sha2::{Digest, Sha256};
use std::{collections::BTreeSet, fmt};

// This unreleased protocol evolves as one coordinated client/backend candidate.
// Optional timing is omitted for legacy observations. Older prerelease decoders
// reject qualified receipts they cannot verify; the old backup protocol is separate.
pub const PROTOCOL_VERSION: u8 = 1;
pub const SEMANTIC_HASH_VERSION: u8 = 1;
pub const MAX_BATCH_BYTES: usize = 256 * 1024;
pub const MAX_EVENT_BYTES: usize = 16 * 1024;
pub const MAX_BATCH_EVENTS: usize = 100;
pub const MAX_ACCOUNT_DOCUMENT_BYTES: usize = 300 * 1024;

/// Errors deliberately contain no submitted text, identifiers or provider data.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ContractError {
    InvalidRequest,
    InvalidRecord,
    BoundsExceeded,
    DuplicateEvent,
    HashMismatch,
    ScopeMismatch,
    ReceiptMismatch,
    NotRetained,
}

impl fmt::Display for ContractError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            Self::InvalidRequest => "usage_analytics_invalid_request",
            Self::InvalidRecord => "usage_analytics_invalid_record",
            Self::BoundsExceeded => "usage_analytics_bounds_exceeded",
            Self::DuplicateEvent => "usage_analytics_duplicate_event",
            Self::HashMismatch => "usage_analytics_hash_mismatch",
            Self::ScopeMismatch => "usage_analytics_scope_mismatch",
            Self::ReceiptMismatch => "usage_analytics_receipt_mismatch",
            Self::NotRetained => "usage_analytics_not_retained",
        })
    }
}

impl std::error::Error for ContractError {}

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

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(try_from = "u8", into = "u8")]
pub struct SemanticHashVersion;

impl TryFrom<u8> for SemanticHashVersion {
    type Error = ContractError;
    fn try_from(value: u8) -> Result<Self, Self::Error> {
        (value == SEMANTIC_HASH_VERSION)
            .then_some(Self)
            .ok_or(ContractError::InvalidRequest)
    }
}

impl From<SemanticHashVersion> for u8 {
    fn from(_: SemanticHashVersion) -> Self {
        SEMANTIC_HASH_VERSION
    }
}

/// Account lifecycle generations include the fresh account epoch zero.
#[derive(Clone, Copy, Debug, Eq, Ord, PartialEq, PartialOrd, Serialize, Deserialize)]
#[serde(try_from = "u64", into = "u64")]
pub struct LifecycleEpoch(u64);
impl LifecycleEpoch {
    pub fn get(self) -> u64 {
        self.0
    }
}
impl TryFrom<u64> for LifecycleEpoch {
    type Error = ContractError;
    fn try_from(value: u64) -> Result<Self, Self::Error> {
        (value <= i64::MAX as u64)
            .then_some(Self(value))
            .ok_or(ContractError::InvalidRequest)
    }
}
impl From<LifecycleEpoch> for u64 {
    fn from(value: LifecycleEpoch) -> Self {
        value.0
    }
}
impl From<Counter> for LifecycleEpoch {
    fn from(value: Counter) -> Self {
        Self(value.get())
    }
}

/// A captured history high-water may be zero for an empty snapshot.
#[derive(Clone, Copy, Debug, Eq, Ord, PartialEq, PartialOrd, Serialize, Deserialize)]
#[serde(try_from = "u64", into = "u64")]
pub struct SequenceBoundary(u64);
impl SequenceBoundary {
    pub fn get(self) -> u64 {
        self.0
    }
}
impl TryFrom<u64> for SequenceBoundary {
    type Error = ContractError;
    fn try_from(value: u64) -> Result<Self, Self::Error> {
        (value <= i64::MAX as u64)
            .then_some(Self(value))
            .ok_or(ContractError::InvalidRequest)
    }
}
impl From<SequenceBoundary> for u64 {
    fn from(value: SequenceBoundary) -> Self {
        value.0
    }
}

/// Positive counters share the signed database range; overflow is rejected.
#[derive(Clone, Copy, Debug, Eq, Ord, PartialEq, PartialOrd, Serialize, Deserialize)]
#[serde(try_from = "u64", into = "u64")]
pub struct Counter(u64);

impl Counter {
    pub fn get(self) -> u64 {
        self.0
    }
}

impl TryFrom<u64> for Counter {
    type Error = ContractError;
    fn try_from(value: u64) -> Result<Self, Self::Error> {
        (value > 0 && value <= i64::MAX as u64)
            .then_some(Self(value))
            .ok_or(ContractError::InvalidRequest)
    }
}

impl From<Counter> for u64 {
    fn from(value: Counter) -> Self {
        value.0
    }
}

#[derive(Clone, Debug, Eq, Ord, PartialEq, PartialOrd, Serialize, Deserialize)]
#[serde(try_from = "String", into = "String")]
pub struct CanonicalUuid(String);

impl CanonicalUuid {
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl TryFrom<String> for CanonicalUuid {
    type Error = ContractError;
    fn try_from(value: String) -> Result<Self, Self::Error> {
        uuid::Uuid::parse_str(&value)
            .is_ok_and(|id| id.to_string() == value)
            .then_some(Self(value))
            .ok_or(ContractError::InvalidRequest)
    }
}

impl From<CanonicalUuid> for String {
    fn from(value: CanonicalUuid) -> Self {
        value.0
    }
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(try_from = "String", into = "String")]
pub struct Sha256Hash(String);

impl Sha256Hash {
    pub fn of(bytes: &[u8]) -> Self {
        Self(format!("{:x}", Sha256::digest(bytes)))
    }
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl TryFrom<String> for Sha256Hash {
    type Error = ContractError;
    fn try_from(value: String) -> Result<Self, Self::Error> {
        (value.len() == 64
            && value
                .bytes()
                .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b)))
        .then_some(Self(value))
        .ok_or(ContractError::InvalidRequest)
    }
}

impl From<Sha256Hash> for String {
    fn from(value: Sha256Hash) -> Self {
        value.0
    }
}

/// Account binding is account owned; source Home is the stable event origin,
/// including after restore onto a different writer Home. Clear changes generation.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Source {
    pub account_binding: CanonicalUuid,
    pub source_home_id: CanonicalUuid,
    pub data_generation: Counter,
}

/// Admission provenance is deliberately absent from stable retained identity.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Provenance {
    pub lifecycle_epoch: LifecycleEpoch,
    pub consent_interval: Counter,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(try_from = "u8", into = "u8")]
pub struct TimingVersion;
impl TryFrom<u8> for TimingVersion {
    type Error = ContractError;
    fn try_from(value: u8) -> Result<Self, Self::Error> {
        (value == 1)
            .then_some(Self)
            .ok_or(ContractError::InvalidRecord)
    }
}
impl From<TimingVersion> for u8 {
    fn from(_: TimingVersion) -> Self {
        1
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TimingBasis {
    MonotonicElapsed,
}

/// A producer observation, never a claim about the server's clock. It stays
/// outside the original projection so existing payload and semantic hashes survive.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct TimingSupplement {
    pub version: TimingVersion,
    pub basis: TimingBasis,
    pub elapsed_ms: SequenceBoundary,
}

impl TimingSupplement {
    #[cfg(test)]
    pub fn monotonic(elapsed_ms: u64) -> Result<Self, ContractError> {
        Ok(Self {
            version: TimingVersion,
            basis: TimingBasis::MonotonicElapsed,
            elapsed_ms: SequenceBoundary::try_from(elapsed_ms)?,
        })
    }

    pub fn validate_projection(&self, value: &Value) -> Result<(), ContractError> {
        if !matches!(
            value.get("event_kind").and_then(Value::as_str),
            Some(
                "provider_request_completed"
                    | "root_run_completed"
                    | "agent_run_completed"
                    | "tool_run_completed"
            )
        ) || value.get("elapsed_ms").and_then(Value::as_u64) != Some(self.elapsed_ms.get())
        {
            return Err(ContractError::InvalidRecord);
        }
        Ok(())
    }

    pub fn sha256(
        &self,
        event_id: &str,
        original_sha256: &Sha256Hash,
    ) -> Result<Sha256Hash, ContractError> {
        #[derive(Serialize)]
        struct Binding<'a> {
            event_id: &'a str,
            original_sha256: &'a Sha256Hash,
            timing: &'a TimingSupplement,
        }
        canonical_hash(
            "cargo-ai-usage-timing-v1",
            &Binding {
                event_id,
                original_sha256,
                timing: self,
            },
        )
    }
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct EventSubmission {
    pub original_text: String,
    pub original_sha256: Sha256Hash,
    pub semantic_hash_version: SemanticHashVersion,
    pub semantic_sha256: Sha256Hash,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub timing: Option<TimingSupplement>,
}

impl EventSubmission {
    /// The caller must first apply the established closed usage projection.
    /// This constructor preserves allowed text and computes its exact identities;
    /// it does not sanitize private local records or provider response bodies.
    pub fn from_allowed_text(original_text: String) -> Result<Self, ContractError> {
        let value = parse_payload_text(&original_text)?;
        let submission = Self {
            original_sha256: Sha256Hash::of(original_text.as_bytes()),
            semantic_hash_version: SemanticHashVersion,
            semantic_sha256: semantic_sha256(&value)?,
            original_text,
            timing: None,
        };
        submission.member()?;
        Ok(submission)
    }

    pub fn from_allowed_text_with_timing(
        original_text: String,
        timing: Option<TimingSupplement>,
    ) -> Result<Self, ContractError> {
        let mut submission = Self::from_allowed_text(original_text)?;
        if timing.is_some() {
            submission.timing = timing;
            submission.member()?;
        }
        Ok(submission)
    }

    /// Extract a wire membership entry while verifying both hashes. Schema and
    /// privacy validation remain the closed projection's separate obligation.
    pub fn member(&self) -> Result<ReceiptMember, ContractError> {
        let value = parse_payload_text(&self.original_text)?;
        if Sha256Hash::of(self.original_text.as_bytes()) != self.original_sha256
            || semantic_sha256(&value)? != self.semantic_sha256
        {
            return Err(ContractError::HashMismatch);
        }
        let event_id = value
            .get("event_id")
            .and_then(Value::as_str)
            .filter(|id| logical_event_id(id))
            .ok_or(ContractError::InvalidRecord)?;
        let timing_sha256 = self
            .timing
            .as_ref()
            .map(|timing| {
                timing.validate_projection(&value)?;
                timing.sha256(event_id, &self.original_sha256)
            })
            .transpose()?;
        Ok(ReceiptMember {
            event_id: event_id.to_owned(),
            semantic_hash_version: self.semantic_hash_version,
            semantic_sha256: self.semantic_sha256.clone(),
            original_sha256: self.original_sha256.clone(),
            timing_sha256,
        })
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

/// Semantic hashing uses sorted object keys and exact integer JSON values.
/// Missing fields, explicit null and array order are intentionally distinct.
pub fn semantic_sha256(value: &Value) -> Result<Sha256Hash, ContractError> {
    canonical_hash("cargo-ai-usage-semantic-v1", value)
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Custody {
    Buffered,
    Retained,
    AnalyticsReady,
}

/// Receipt entries are fully ordered, with both content identities. A count or
/// batch digest alone cannot certify retention of a missing or changed member.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ReceiptMember {
    pub event_id: String,
    pub semantic_hash_version: SemanticHashVersion,
    pub semantic_sha256: Sha256Hash,
    pub original_sha256: Sha256Hash,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub timing_sha256: Option<Sha256Hash>,
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

fn logical_event_id(value: &str) -> bool {
    let canonical = |text: &str| uuid::Uuid::parse_str(text).is_ok_and(|id| id.to_string() == text);
    canonical(value)
        || ["cai_run_", "cai_agent_run_", "cai_event_"]
            .iter()
            .any(|prefix| value.strip_prefix(prefix).is_some_and(canonical))
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

/// Decode bounded exact payload text without integer rounding, duplicate object
/// keys, or missing/null conflation. Closed schema validation is separate.
pub fn parse_payload_text(text: &str) -> Result<Value, ContractError> {
    if text.len() > MAX_EVENT_BYTES {
        return Err(ContractError::BoundsExceeded);
    }
    parse_exact_json(text).map_err(|_| ContractError::InvalidRecord)
}

/// Exact syntax for an account envelope. Route-specific closed fields and
/// authentication are validated separately; legacy routes retain their parser.
pub fn parse_account_document(text: &str) -> Result<Value, ContractError> {
    if text.len() >= MAX_ACCOUNT_DOCUMENT_BYTES {
        return Err(ContractError::BoundsExceeded);
    }
    parse_exact_json(text).map_err(|_| ContractError::InvalidRequest)
}

pub fn canonical_hash<T: Serialize>(domain: &str, value: &T) -> Result<Sha256Hash, ContractError> {
    let value = serde_json::to_value(value).map_err(|_| ContractError::InvalidRecord)?;
    let mut bytes = domain.as_bytes().to_vec();
    bytes.push(0);
    canonical_bytes(&value, &mut bytes)?;
    Ok(Sha256Hash::of(&bytes))
}

fn canonical_bytes(value: &Value, bytes: &mut Vec<u8>) -> Result<(), ContractError> {
    match value {
        Value::Object(object) => {
            bytes.push(b'{');
            let mut keys: Vec<_> = object.keys().collect();
            keys.sort_unstable();
            for (index, key) in keys.into_iter().enumerate() {
                if index > 0 {
                    bytes.push(b',');
                }
                serde_json::to_writer(&mut *bytes, key)
                    .map_err(|_| ContractError::InvalidRecord)?;
                bytes.push(b':');
                canonical_bytes(&object[key], bytes)?;
            }
            bytes.push(b'}');
        }
        Value::Array(array) => {
            bytes.push(b'[');
            for (index, item) in array.iter().enumerate() {
                if index > 0 {
                    bytes.push(b',');
                }
                canonical_bytes(item, bytes)?;
            }
            bytes.push(b']');
        }
        _ => serde_json::to_writer(bytes, value).map_err(|_| ContractError::InvalidRecord)?,
    }
    Ok(())
}

/// Reject duplicate keys and noninteger numeric coercions at every nesting depth.
fn parse_exact_json(text: &str) -> Result<Value, serde_json::Error> {
    struct Exact(Value);
    impl<'de> Deserialize<'de> for Exact {
        fn deserialize<D: de::Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
            struct Visitor;
            impl<'de> de::Visitor<'de> for Visitor {
                type Value = Exact;
                fn expecting(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
                    f.write_str("exact JSON")
                }
                fn visit_bool<E: de::Error>(self, value: bool) -> Result<Exact, E> {
                    Ok(Exact(Value::Bool(value)))
                }
                fn visit_i64<E: de::Error>(self, value: i64) -> Result<Exact, E> {
                    Ok(Exact(value.into()))
                }
                fn visit_u64<E: de::Error>(self, value: u64) -> Result<Exact, E> {
                    Ok(Exact(value.into()))
                }
                fn visit_f64<E: de::Error>(self, _: f64) -> Result<Exact, E> {
                    Err(E::custom("noninteger number"))
                }
                fn visit_str<E: de::Error>(self, value: &str) -> Result<Exact, E> {
                    Ok(Exact(value.into()))
                }
                fn visit_string<E: de::Error>(self, value: String) -> Result<Exact, E> {
                    Ok(Exact(value.into()))
                }
                fn visit_unit<E: de::Error>(self) -> Result<Exact, E> {
                    Ok(Exact(Value::Null))
                }
                fn visit_seq<A: de::SeqAccess<'de>>(self, mut seq: A) -> Result<Exact, A::Error> {
                    let mut array = Vec::new();
                    while let Some(Exact(value)) = seq.next_element()? {
                        array.push(value);
                    }
                    Ok(Exact(Value::Array(array)))
                }
                fn visit_map<A: de::MapAccess<'de>>(self, mut map: A) -> Result<Exact, A::Error> {
                    let mut object = serde_json::Map::new();
                    while let Some((key, Exact(value))) = map.next_entry::<String, Exact>()? {
                        if object.insert(key, value).is_some() {
                            return Err(de::Error::custom("duplicate field"));
                        }
                    }
                    Ok(Exact(Value::Object(object)))
                }
            }
            deserializer.deserialize_any(Visitor)
        }
    }
    let mut deserializer = serde_json::Deserializer::from_str(text);
    let Exact(value) = Exact::deserialize(&mut deserializer)?;
    deserializer.end()?;
    Ok(value)
}
