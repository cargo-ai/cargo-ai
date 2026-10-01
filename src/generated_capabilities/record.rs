//! Portable framing for a generated runtime's exact invocation compatibility.

pub const OPEN: [u8; 16] = [
    0, 217, 82, 166, 31, 240, 75, 136, 3, 193, 109, 229, 56, 154, 7, 252,
];
pub const CLOSE: [u8; 16] = [
    253, 6, 155, 57, 228, 108, 192, 2, 137, 74, 241, 30, 167, 83, 216, 0,
];
pub const IDENTITY: &[u8; 26] = b"cargo-ai.generated-runtime";
pub const CLI_IDENTITY: &[u8; 26] = b"cargo-ai.cli-run-runtime\0\0";
pub const REVISION: u32 = 2;
// Three invocation flags and four tagged setting forms are required together.
pub const THINKING_CAPABILITIES: u32 = 0b111_1111;
pub const RECORD_LEN: usize = OPEN.len() + IDENTITY.len() + 8 + CLOSE.len();

/// Emits numeric source data; template/source text is not a binary declaration.
pub fn encoded_record(cli_run: bool) -> Vec<u8> {
    let mut record = Vec::with_capacity(RECORD_LEN);
    // Keep source-reader implementations from embedding a contiguous declaration
    // through constant folding. Only generated provenance emits the whole frame.
    record.extend_from_slice(std::hint::black_box(&OPEN));
    record.extend_from_slice(if cli_run { CLI_IDENTITY } else { IDENTITY });
    record.extend_from_slice(&REVISION.to_le_bytes());
    record.extend_from_slice(&THINKING_CAPABILITIES.to_le_bytes());
    record.extend_from_slice(std::hint::black_box(&CLOSE));
    record
}
