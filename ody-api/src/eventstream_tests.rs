//! Tests for the AWS event stream frame decoder.

use super::*;

#[test]
fn crc32_matches_ieee_reference_values() {
    // Well-known CRC-32 check values.
    assert_eq!(crc32(b""), 0);
    assert_eq!(crc32(b"123456789"), 0xCBF4_3926);
    assert_eq!(crc32(b"The quick brown fox jumps over the lazy dog"), 0x414F_A339);
}

#[test]
fn decodes_single_frame() {
    let payload = br#"{"bytes":"eyJ0eXBlIjoicGluZyJ9"}"#;
    let frame = encode_frame(&[], payload);
    let mut decoder = EventStreamDecoder::new();
    let payloads = decoder.feed(&frame).expect("decodes");
    assert_eq!(payloads, vec![payload.to_vec()]);
    assert!(decoder.is_idle());
}

#[test]
fn decodes_multiple_frames_in_one_chunk() {
    let f1 = encode_frame(&[], br#"{"bytes":"YQ=="}"#);
    let f2 = encode_frame(&[], br#"{"bytes":"Yg=="}"#);
    let mut chunk = f1.clone();
    chunk.extend_from_slice(&f2);
    let mut decoder = EventStreamDecoder::new();
    let payloads = decoder.feed(&chunk).expect("decodes");
    assert_eq!(payloads.len(), 2);
    assert!(decoder.is_idle());
}

#[test]
fn decodes_frames_split_across_chunks() {
    let frame = encode_frame(&[], br#"{"bytes":"Yw=="}"#);
    let mut decoder = EventStreamDecoder::new();
    let split = frame.len() / 2;
    let first = decoder.feed(&frame[..split]).expect("first chunk");
    assert!(first.is_empty());
    let second = decoder.feed(&frame[split..]).expect("second chunk");
    assert_eq!(second, vec![br#"{"bytes":"Yw=="}"#.to_vec()]);
}

#[test]
fn skips_headers() {
    // Two headers: ":event-type" (string) and ":message-type" (string).
    let mut headers = Vec::new();
    headers.push(11u8); // name len
    headers.extend_from_slice(b":event-type");
    headers.push(0x07); // string value
    headers.push(7u8); // value len
    headers.extend_from_slice(b"message");
    headers.push(12u8);
    headers.extend_from_slice(b":message-type");
    headers.push(0x07);
    headers.push(7u8);
    headers.extend_from_slice(b"message");
    let payload = br#"{"bytes":"ZA=="}"#;
    let frame = encode_frame(&headers, payload);
    let mut decoder = EventStreamDecoder::new();
    let payloads = decoder.feed(&frame).expect("decodes");
    assert_eq!(payloads, vec![payload.to_vec()]);
}

#[test]
fn rejects_corrupt_prelude_crc() {
    let mut frame = encode_frame(&[], br#"{"bytes":"ZQ=="}"#);
    frame[8] ^= 0xFF;
    let mut decoder = EventStreamDecoder::new();
    let err = decoder.feed(&frame).expect_err("CRC must fail");
    assert!(err.to_string().contains("prelude CRC"), "{err}");
}

#[test]
fn rejects_corrupt_message_crc() {
    let mut frame = encode_frame(&[], br#"{"bytes":"Zg=="}"#);
    let n = frame.len();
    frame[n - 1] ^= 0xFF;
    let mut decoder = EventStreamDecoder::new();
    let err = decoder.feed(&frame).expect_err("CRC must fail");
    assert!(err.to_string().contains("message CRC"), "{err}");
}

#[test]
fn rejects_impossible_frame_length() {
    let mut decoder = EventStreamDecoder::new();
    // total_len = 4 is smaller than the minimum frame size.
    let err = decoder
        .feed(&[0, 0, 0, 4, 0, 0, 0, 0, 0, 0, 0, 0])
        .expect_err("length must fail");
    assert!(err.to_string().contains("frame length"), "{err}");
}

#[test]
fn extracts_base64_event_bytes() {
    // base64 of {"type":"ping"}
    let payload = br#"{"bytes":"eyJ0eXBlIjoicGluZyJ9"}"#;
    let bytes = eventstream_payload_bytes(payload).expect("extracts");
    assert_eq!(bytes, br#"{"type":"ping"}"#.to_vec());
}

#[test]
fn rejects_payload_without_bytes_field() {
    let err = eventstream_payload_bytes(br#"{"other":1}"#).expect_err("missing bytes");
    assert!(err.to_string().contains("bytes"), "{err}");
}
