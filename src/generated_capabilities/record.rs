//! Portable framing for a generated runtime's exact invocation compatibility.

pub const OPEN: [u8; 16] = [
    0, 217, 82, 166, 31, 240, 75, 136, 3, 193, 109, 229, 56, 154, 7, 252,
];
pub const CLOSE: [u8; 16] = [
    253, 6, 155, 57, 228, 108, 192, 2, 137, 74, 241, 30, 167, 83, 216, 0,
];
pub const IDENTITY: &[u8; 26] = b"cargo-ai.generated-runtime";
pub const CLI_IDENTITY: &[u8; 26] = b"cargo-ai.cli-run-runtime\0\0";
pub const REVISION: u32 = 6;
// Thinking controls and versioned root/media/descendant policy enforcement
// are required together for this declaration revision.
pub const THINKING_CAPABILITIES: u32 = 0b111_1111_1111;
pub const DEFINITION_OPEN: [u8; 16] = [
    17, 209, 42, 66, 171, 99, 187, 13, 214, 3, 168, 194, 51, 17, 222, 31,
];
pub const DEFINITION_CLOSE: [u8; 16] = [
    32, 221, 18, 52, 193, 169, 4, 215, 14, 188, 100, 172, 67, 43, 210, 18,
];
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
