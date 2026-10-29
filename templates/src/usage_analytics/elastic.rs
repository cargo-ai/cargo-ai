//! Bounded durable-upload protocol and one-frame incremental Zstandard batches.
use super::contract::{
    canonical_hash, CanonicalUuid, ContractError, EventSubmission, LifecycleEpoch, Provenance,
    ReceiptMember, Sha256Hash, Source,
};
use base64::{engine::general_purpose::STANDARD, Engine as _};
use serde::{de, Deserialize, Serialize};
use std::{
    collections::BTreeSet,
    io::{BufRead, BufReader, Cursor, Read, Write},
    str::FromStr,
};

pub const PROTOCOL_VERSION: u8 = 2;
pub const MAX_BATCH_EVENTS: usize = 4096;
pub const INPUT_GROUP_BYTES: usize = 128 * 1024;
pub const DEFAULT_TARGET_BYTES: usize = 2_000_000;
pub const MAX_COMPRESSED_BYTES: usize = 3_000_000;
pub const MAX_DECODED_BYTES: usize = 8_000_000;
pub const MAX_ACCOUNT_BYTES: usize = 4_500_000;
const MAX_LINE_BYTES: usize = 64 * 1024;
const FINALIZATION_RESERVE: usize = 64 * 1024;

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct Version;
impl Serialize for Version {
    fn serialize<S: serde::Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        serializer.serialize_u8(PROTOCOL_VERSION)
    }
}
impl<'de> Deserialize<'de> for Version {
    fn deserialize<D: serde::Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        (u8::deserialize(deserializer)? == PROTOCOL_VERSION)
            .then_some(Self)
            .ok_or_else(|| de::Error::custom("unsupported usage upload protocol"))
    }
}
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct EncodedBatch {
    pub data_base64: String,
    pub encoded_sha256: Sha256Hash,
    pub event_count: u32,
    pub decoded_bytes: u32,
}
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(tag = "command", rename_all = "snake_case", deny_unknown_fields)]
pub enum Request {
    Status {
        protocol_version: Version,
    },
    Enable {
        protocol_version: Version,
        source_home_id: CanonicalUuid,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        replace_home_id: Option<CanonicalUuid>,
    },
    Disable {
        protocol_version: Version,
        source: Source,
        provenance: Provenance,
    },
    Upload {
        protocol_version: Version,
        source: Source,
        provenance: Provenance,
        batch_id: CanonicalUuid,
        ordered_membership_sha256: Sha256Hash,
        batch: EncodedBatch,
        retry_count: u32,
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
        after_sequence: Option<u64>,
        limit: u16,
    },
}
impl FromStr for Request {
    type Err = ContractError;
    fn from_str(text: &str) -> Result<Self, Self::Err> {
        if text.len() > MAX_ACCOUNT_BYTES {
            return Err(ContractError::BoundsExceeded);
        }
        let request: Self =
            serde_json::from_str(text).map_err(|_| ContractError::InvalidRequest)?;
        request.validate()?;
        Ok(request)
    }
}
impl Request {
    pub fn validate(&self) -> Result<(), ContractError> {
        match self {
            Self::Upload {
                batch, retry_count, ..
            } if batch.data_base64.len() > MAX_COMPRESSED_BYTES.div_ceil(3) * 4
                || batch.event_count == 0
                || batch.event_count as usize > MAX_BATCH_EVENTS
                || batch.decoded_bytes as usize > MAX_DECODED_BYTES
                || *retry_count > 1_000_000 =>
            {
                return Err(ContractError::BoundsExceeded);
            }
            Self::History { limit, .. } if *limit == 0 || *limit > 100 => {
                return Err(ContractError::BoundsExceeded);
            }
            _ => {}
        }
        Ok(())
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ProcessingStatus {
    NotObserved,
}

/// These fields attest only the submitted batch's durable buffer acknowledgment.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct UploadAck {
    pub protocol_version: Version,
    pub source: Source,
    pub provenance: Provenance,
    pub batch_id: CanonicalUuid,
    pub ordered_membership_sha256: Sha256Hash,
    pub encoded_sha256: Sha256Hash,
    pub acknowledged_events: u32,
    pub durable_acknowledged: bool,
    pub processing_status: ProcessingStatus,
}
impl UploadAck {
    pub fn verify(&self, request: &Request) -> Result<(), ContractError> {
        let Request::Upload {
            source,
            provenance,
            batch_id,
            ordered_membership_sha256,
            batch,
            ..
        } = request
        else {
            return Err(ContractError::InvalidRequest);
        };
        if self.source != *source
            || self.provenance != *provenance
            || self.batch_id != *batch_id
            || self.ordered_membership_sha256 != *ordered_membership_sha256
            || self.encoded_sha256 != batch.encoded_sha256
            || self.acknowledged_events != batch.event_count
            || !self.durable_acknowledged
        {
            return Err(ContractError::ReceiptMismatch);
        }
        Ok(())
    }
}

pub fn membership_sha256(
    source: &Source,
    members: &[ReceiptMember],
) -> Result<Sha256Hash, ContractError> {
    if members.is_empty() || members.len() > MAX_BATCH_EVENTS {
        return Err(ContractError::BoundsExceeded);
    }
    let mut ids = BTreeSet::new();
    for member in members {
        if !ids.insert(&member.event_id) {
            return Err(ContractError::DuplicateEvent);
        }
    }
    canonical_hash("cargo-ai-usage-upload-membership-v2", &(source, members))
}

/// Returns the exact compressed request prefix; remaining events stay eligible.
pub fn encode_batch(
    events: &[EventSubmission],
    target_bytes: usize,
) -> Result<(EncodedBatch, usize), ContractError> {
    if events.is_empty() || target_bytes == 0 || target_bytes > MAX_COMPRESSED_BYTES {
        return Err(ContractError::BoundsExceeded);
    }
    let target = target_bytes.min(MAX_COMPRESSED_BYTES - FINALIZATION_RESERVE);
    let mut encoder =
        zstd::stream::Encoder::new(Vec::new(), 3).map_err(|_| ContractError::InvalidRecord)?;
    let mut group = Vec::with_capacity(INPUT_GROUP_BYTES);
    let mut decoded = 0usize;
    let mut accepted = 0usize;
    for event in events.iter().take(MAX_BATCH_EVENTS) {
        event.member()?;
        let mut line = serde_json::to_vec(event).map_err(|_| ContractError::InvalidRecord)?;
        line.push(b'\n');
        if line.len() > MAX_LINE_BYTES {
            return Err(ContractError::BoundsExceeded);
        }
        if decoded + line.len() > MAX_DECODED_BYTES {
            break;
        }
        if group.len() + line.len() > INPUT_GROUP_BYTES && !group.is_empty() {
            encoder
                .write_all(&group)
                .and_then(|_| encoder.flush())
                .map_err(|_| ContractError::BoundsExceeded)?;
            group.clear();
            if encoder.get_ref().len() >= target {
                break;
            }
        }
        group.extend_from_slice(&line);
        decoded += line.len();
        accepted += 1;
    }
    if accepted == 0 {
        return Err(ContractError::BoundsExceeded);
    }
    encoder
        .write_all(&group)
        .map_err(|_| ContractError::BoundsExceeded)?;
    let bytes = encoder
        .finish()
        .map_err(|_| ContractError::BoundsExceeded)?;
    if bytes.len() > MAX_COMPRESSED_BYTES {
        if accepted == 1 {
            return Err(ContractError::BoundsExceeded);
        }
        return encode_batch(&events[..accepted / 2], target_bytes);
    }
    Ok((
        EncodedBatch {
            encoded_sha256: Sha256Hash::of(&bytes),
            data_base64: STANDARD.encode(bytes),
            event_count: accepted as u32,
            decoded_bytes: decoded as u32,
        },
        accepted,
    ))
}

/// Decode a single bounded frame, validating each complete event before forwarding.
impl TryFrom<&EncodedBatch> for Vec<EventSubmission> {
    type Error = ContractError;
    fn try_from(batch: &EncodedBatch) -> Result<Self, Self::Error> {
        if batch.data_base64.len() > MAX_COMPRESSED_BYTES.div_ceil(3) * 4
            || batch.event_count == 0
            || batch.event_count as usize > MAX_BATCH_EVENTS
            || batch.decoded_bytes as usize > MAX_DECODED_BYTES
        {
            return Err(ContractError::BoundsExceeded);
        }
        let bytes = STANDARD
            .decode(&batch.data_base64)
            .map_err(|_| ContractError::InvalidRecord)?;
        if bytes.len() > MAX_COMPRESSED_BYTES
            || !bytes.starts_with(&[0x28, 0xb5, 0x2f, 0xfd])
            || Sha256Hash::of(&bytes) != batch.encoded_sha256
        {
            return Err(ContractError::HashMismatch);
        }
        let encoded_len = bytes.len();
        let mut decoder = zstd::stream::Decoder::with_buffer(Cursor::new(bytes))
            .map_err(|_| ContractError::InvalidRecord)?
            .single_frame();
        decoder
            .window_log_max(23)
            .map_err(|_| ContractError::BoundsExceeded)?;
        let mut reader = BufReader::with_capacity(INPUT_GROUP_BYTES, decoder);
        let mut events = Vec::new();
        let mut decoded = 0usize;
        let mut ids = BTreeSet::new();
        loop {
            let mut line = Vec::new();
            let read = reader
                .by_ref()
                .take((MAX_LINE_BYTES + 1) as u64)
                .read_until(b'\n', &mut line)
                .map_err(|_| ContractError::InvalidRecord)?;
            if read == 0 {
                break;
            }
            decoded = decoded
                .checked_add(read)
                .ok_or(ContractError::BoundsExceeded)?;
            if line.len() > MAX_LINE_BYTES
                || decoded > MAX_DECODED_BYTES
                || events.len() >= MAX_BATCH_EVENTS
                || line.last() != Some(&b'\n')
            {
                return Err(ContractError::BoundsExceeded);
            }
            line.pop();
            if line.is_empty() {
                return Err(ContractError::InvalidRecord);
            }
            let text = std::str::from_utf8(&line).map_err(|_| ContractError::InvalidRecord)?;
            let event: EventSubmission =
                serde_json::from_str(text).map_err(|_| ContractError::InvalidRecord)?;
            let member = event.member()?;
            if !ids.insert(member.event_id) {
                return Err(ContractError::DuplicateEvent);
            }
            events.push(event);
        }
        let cursor = reader.into_inner().finish();
        if cursor.position() as usize != encoded_len
            || decoded != batch.decoded_bytes as usize
            || events.len() != batch.event_count as usize
        {
            return Err(ContractError::InvalidRecord);
        }
        Ok(events)
    }
}

#[cfg(test)]
pub fn decode_batch(batch: &EncodedBatch) -> Result<Vec<EventSubmission>, ContractError> {
    Vec::try_from(batch)
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    fn uuid(n: u64) -> CanonicalUuid {
        CanonicalUuid::try_from(format!("00000000-0000-4000-8000-{n:012}")).unwrap()
    }
    fn source() -> Source {
        Source {
            account_binding: uuid(1),
            source_home_id: uuid(2),
            data_generation: super::super::contract::Counter::try_from(1).unwrap(),
        }
    }
    fn event(n: u64) -> EventSubmission {
        EventSubmission::from_allowed_text(json!({"schema_version":1,"normalization_version":1,"event_id":uuid(n+100).as_str(),"root_run_id":uuid(10).as_str(),"agent_run_id":uuid(11).as_str(),"event_type":"provider_request","event_kind":"provider_request_completed","occurred_at_ms":1,"tokens":{"input_tokens":100,"output_tokens":5,"total_tokens":105}}).to_string()).unwrap()
    }
    #[test]
    fn single_stream_roundtrip_exceeds_old_small_batch_and_preserves_exact_events() {
        let events: Vec<_> = (0..512).map(event).collect();
        let (encoded, n) = encode_batch(&events, DEFAULT_TARGET_BYTES).unwrap();
        assert_eq!(n, events.len());
        assert_eq!(decode_batch(&encoded).unwrap(), events);
        assert!(STANDARD.decode(encoded.data_base64).unwrap().len() < MAX_COMPRESSED_BYTES);
    }
    #[test]
    fn target_stops_at_complete_event_boundary_and_leaves_a_prefix() {
        let events: Vec<_> = (0..1000).map(event).collect();
        let (encoded, n) = encode_batch(&events, 1).unwrap();
        assert!(n > 0 && n < events.len());
        assert_eq!(decode_batch(&encoded).unwrap(), events[..n]);
    }
    #[test]
    fn untrusted_counts_hash_and_expansion_are_rejected() {
        let (batch, _) = encode_batch(&[event(1)], DEFAULT_TARGET_BYTES).unwrap();
        let mut value = batch.clone();
        value.event_count = 2;
        assert!(decode_batch(&value).is_err());
        value = batch.clone();
        value.decoded_bytes += 1;
        assert!(decode_batch(&value).is_err());
        value = batch;
        value.encoded_sha256 = Sha256Hash::of(b"different");
        assert!(decode_batch(&value).is_err());
        let bytes =
            zstd::stream::encode_all(Cursor::new(vec![b' '; MAX_DECODED_BYTES + 1]), 3).unwrap();
        let bomb = EncodedBatch {
            data_base64: STANDARD.encode(&bytes),
            encoded_sha256: Sha256Hash::of(&bytes),
            event_count: 1,
            decoded_bytes: 1,
        };
        assert!(decode_batch(&bomb).is_err());
    }
    #[test]
    fn concatenated_frames_and_trailing_bytes_are_not_a_batch() {
        let (batch, _) = encode_batch(&[event(1)], DEFAULT_TARGET_BYTES).unwrap();
        let bytes = STANDARD.decode(&batch.data_base64).unwrap();
        for suffix in [&bytes[..], b"trailing".as_slice()] {
            let mut joined = bytes.clone();
            joined.extend_from_slice(suffix);
            let value = EncodedBatch {
                data_base64: STANDARD.encode(&joined),
                encoded_sha256: Sha256Hash::of(&joined),
                ..batch.clone()
            };
            assert!(decode_batch(&value).is_err());
        }
    }
    #[test]
    fn invalid_event_duplicate_members_and_duplicate_request_fields_fail() {
        let events = vec![event(1), event(1)];
        let (encoded, _) = encode_batch(&events, DEFAULT_TARGET_BYTES).unwrap();
        assert_eq!(decode_batch(&encoded), Err(ContractError::DuplicateEvent));
        let members = events
            .iter()
            .map(|e| e.member().unwrap())
            .collect::<Vec<_>>();
        assert_eq!(
            membership_sha256(&source(), &members),
            Err(ContractError::DuplicateEvent)
        );
        assert!(Request::from_str(
            "{\"command\":\"status\",\"protocol_version\":2,\"protocol_version\":2}"
        )
        .is_err());
        assert!(Request::from_str("{\"command\":\"status\",\"protocol_version\":1}").is_err());
        assert!(Request::from_str(
            "{\"command\":\"status\",\"protocol_version\":2,\"account\":\"forged\"}"
        )
        .is_err());
    }
    #[test]
    fn acknowledgment_is_exact_durable_batch_and_does_not_assert_query_readiness() {
        let (batch, _) = encode_batch(&[event(1)], DEFAULT_TARGET_BYTES).unwrap();
        let source = source();
        let members = vec![event(1).member().unwrap()];
        let membership = membership_sha256(&source, &members).unwrap();
        let provenance = Provenance {
            lifecycle_epoch: LifecycleEpoch::try_from(0).unwrap(),
            consent_interval: super::super::contract::Counter::try_from(1).unwrap(),
        };
        let request = Request::Upload {
            protocol_version: Version,
            source: source.clone(),
            provenance: provenance.clone(),
            batch_id: uuid(3),
            ordered_membership_sha256: membership.clone(),
            batch: batch.clone(),
            retry_count: 0,
        };
        let mut ack = UploadAck {
            protocol_version: Version,
            source,
            provenance,
            batch_id: uuid(3),
            ordered_membership_sha256: membership,
            encoded_sha256: batch.encoded_sha256,
            acknowledged_events: 1,
            durable_acknowledged: true,
            processing_status: ProcessingStatus::NotObserved,
        };
        assert!(ack.verify(&request).is_ok());
        ack.acknowledged_events = 2;
        assert!(ack.verify(&request).is_err());
        ack.acknowledged_events = 1;
        ack.durable_acknowledged = false;
        assert!(ack.verify(&request).is_err());
    }
}
