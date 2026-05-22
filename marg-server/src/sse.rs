use bytes::{Buf, BytesMut};

use marg_providers::ChatUsage;

pub fn take_event(buf: &mut BytesMut) -> Option<Vec<u8>> {
    let bytes = buf.as_ref();
    let mut boundary: Option<(usize, usize)> = None;
    let mut i = 0;
    while i + 1 < bytes.len() {
        if bytes[i] == b'\n' && bytes[i + 1] == b'\n' {
            boundary = Some((i, 2));
            break;
        }
        if i + 3 < bytes.len()
            && bytes[i] == b'\r'
            && bytes[i + 1] == b'\n'
            && bytes[i + 2] == b'\r'
            && bytes[i + 3] == b'\n'
        {
            boundary = Some((i, 4));
            break;
        }
        i += 1;
    }
    let (pos, len) = boundary?;
    let mut event = Vec::with_capacity(pos);
    event.extend_from_slice(&buf[..pos]);
    buf.advance(pos + len);
    Some(event)
}

pub fn parse_usage(event: &[u8]) -> Option<ChatUsage> {
    let text = std::str::from_utf8(event).ok()?;
    for line in text.split('\n') {
        let line = line.trim_end_matches('\r');
        let Some(rest) = line.strip_prefix("data:") else { continue };
        let rest = rest.trim_start();
        if rest == "[DONE]" {
            return None;
        }
        let Ok(value) = serde_json::from_str::<serde_json::Value>(rest) else { continue };
        if let Some(usage_value) = value.get("usage") {
            if usage_value.is_object() {
                if let Ok(usage) = serde_json::from_value::<ChatUsage>(usage_value.clone()) {
                    return Some(usage);
                }
            }
        }
    }
    None
}
