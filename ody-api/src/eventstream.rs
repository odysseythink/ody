//! Minimal AWS event stream frame decoder.
//!
//! AWS Bedrock streaming operations (`invoke-with-response-stream`) frame
//! their response bodies as event stream messages: a 12-byte prelude
//! (total length, headers length, prelude CRC), opaque headers, a payload,
//! and a trailing message CRC. Each payload is JSON of the form
//! `{ "bytes": "<base64 anthropic event JSON>" }`.
//!
//! The workspace has no event stream dependency (only `aws-sigv4` for
//! signing), so the framing is decoded here. CRC32 is the IEEE variant
//! (polynomial 0xEDB88320), matching AWS event stream checksums.

use crate::error::ApiError;

const PRELUDE_LEN: usize = 12;
const CRC_LEN: usize = 4;

/// Streaming decoder: feed raw response bytes, receive complete payloads.
#[derive(Debug, Default)]
pub(crate) struct EventStreamDecoder {
    buffer: Vec<u8>,
}

impl EventStreamDecoder {
    pub(crate) fn new() -> Self {
        Self::default()
    }

    /// Feed a chunk of the response body. Returns every frame payload
    /// completed by this chunk, in order.
    pub(crate) fn feed(&mut self, chunk: &[u8]) -> Result<Vec<Vec<u8>>, ApiError> {
        self.buffer.extend_from_slice(chunk);
        let mut payloads = Vec::new();
        loop {
            let Some(frame_len) = self.frame_length()? else {
                break;
            };
            let frame: Vec<u8> = self.buffer.drain(..frame_len).collect();
            payloads.push(decode_frame(&frame)?);
        }
        Ok(payloads)
    }

    /// Length of the first buffered frame if it is complete yet.
    fn frame_length(&self) -> Result<Option<usize>, ApiError> {
        if self.buffer.len() < PRELUDE_LEN {
            return Ok(None);
        }
        let total_len = u32::from_be_bytes([
            self.buffer[0],
            self.buffer[1],
            self.buffer[2],
            self.buffer[3],
        ]) as usize;
        if total_len < PRELUDE_LEN + CRC_LEN {
            return Err(ApiError::Stream(format!(
                "invalid event stream frame length {total_len}"
            )));
        }
        if self.buffer.len() < total_len {
            return Ok(None);
        }
        Ok(Some(total_len))
    }

    /// True when no partial frame is buffered (end-of-stream sanity check).
    #[cfg(test)]
    pub(crate) fn is_idle(&self) -> bool {
        self.buffer.is_empty()
    }
}

/// Decode and validate one complete frame, returning its payload.
fn decode_frame(frame: &[u8]) -> Result<Vec<u8>, ApiError> {
    let total_len = frame.len();
    let prelude_crc = u32::from_be_bytes([frame[8], frame[9], frame[10], frame[11]]);
    if crc32(&frame[..8]) != prelude_crc {
        return Err(ApiError::Stream(
            "event stream prelude CRC mismatch".into(),
        ));
    }
    let headers_len = u32::from_be_bytes([frame[4], frame[5], frame[6], frame[7]]) as usize;
    let message_crc = u32::from_be_bytes([
        frame[total_len - 4],
        frame[total_len - 3],
        frame[total_len - 2],
        frame[total_len - 1],
    ]);
    if crc32(&frame[..total_len - CRC_LEN]) != message_crc {
        return Err(ApiError::Stream(
            "event stream message CRC mismatch".into(),
        ));
    }
    let payload_start = PRELUDE_LEN + headers_len;
    let payload_end = total_len - CRC_LEN;
    if payload_start > payload_end {
        return Err(ApiError::Stream(
            "event stream frame headers overflow payload".into(),
        ));
    }
    Ok(frame[payload_start..payload_end].to_vec())
}

/// Extract the inner Anthropic event JSON from an event stream payload of the
/// form `{ "bytes": "<base64>" }`.
pub(crate) fn eventstream_payload_bytes(payload: &[u8]) -> Result<Vec<u8>, ApiError> {
    let value: serde_json::Value =
        serde_json::from_slice(payload).map_err(|e| ApiError::Stream(format!("event stream payload is not JSON: {e}")))?;
    let encoded = value
        .get("bytes")
        .and_then(serde_json::Value::as_str)
        .ok_or_else(|| ApiError::Stream("event stream payload missing bytes field".into()))?;
    base64::Engine::decode(
        &base64::engine::general_purpose::STANDARD,
        encoded,
    )
    .map_err(|e| ApiError::Stream(format!("event stream payload bytes is not base64: {e}")))
}

/// CRC-32 (IEEE 802.3, polynomial 0xEDB88320), table-based.
pub(crate) fn crc32(bytes: &[u8]) -> u32 {
    const TABLE: [u32; 256] = build_crc32_table();
    let mut crc = 0xFFFF_FFFFu32;
    for &byte in bytes {
        crc = TABLE[((crc ^ byte as u32) & 0xFF) as usize] ^ (crc >> 8);
    }
    crc ^ 0xFFFF_FFFF
}

const fn build_crc32_table() -> [u32; 256] {
    let mut table = [0u32; 256];
    let mut i = 0;
    while i < 256 {
        let mut c = i as u32;
        let mut k = 0;
        while k < 8 {
            c = if c & 1 != 0 { 0xEDB8_8320 ^ (c >> 1) } else { c >> 1 };
            k += 1;
        }
        table[i] = c;
        i += 1;
    }
    table
}

/// Encode a frame for tests (and documentation of the wire shape).
#[cfg(test)]
pub(crate) fn encode_frame(headers: &[u8], payload: &[u8]) -> Vec<u8> {
    let total_len = (PRELUDE_LEN + headers.len() + payload.len() + CRC_LEN) as u32;
    let mut frame = Vec::new();
    frame.extend_from_slice(&total_len.to_be_bytes());
    frame.extend_from_slice(&(headers.len() as u32).to_be_bytes());
    frame.extend_from_slice(&crc32(&frame[..8]).to_be_bytes());
    frame.extend_from_slice(headers);
    frame.extend_from_slice(payload);
    let crc = crc32(&frame);
    frame.extend_from_slice(&crc.to_be_bytes());
    frame
}

#[cfg(test)]
#[path = "eventstream_tests.rs"]
mod eventstream_tests;
