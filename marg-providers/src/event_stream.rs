use bytes::BytesMut;

#[derive(Debug, Clone)]
pub struct EventStreamMessage {
    pub headers: Vec<(String, EventHeaderValue)>,
    pub payload: Vec<u8>,
}

#[derive(Debug, Clone)]
pub enum EventHeaderValue {
    String(String),
    Other,
}

impl EventStreamMessage {
    pub fn header_str(&self, name: &str) -> Option<&str> {
        self.headers.iter().find_map(|(n, v)| {
            if n == name {
                if let EventHeaderValue::String(s) = v {
                    return Some(s.as_str());
                }
            }
            None
        })
    }
}

pub fn try_decode(buf: &mut BytesMut) -> Result<Option<EventStreamMessage>, String> {
    if buf.len() < 16 {
        return Ok(None);
    }
    let total_len = u32::from_be_bytes([buf[0], buf[1], buf[2], buf[3]]) as usize;
    if total_len < 16 || total_len > 16 * 1024 * 1024 {
        return Err(format!("event-stream frame length out of range: {}", total_len));
    }
    if buf.len() < total_len {
        return Ok(None);
    }
    let headers_len = u32::from_be_bytes([buf[4], buf[5], buf[6], buf[7]]) as usize;
    if headers_len + 12 + 4 > total_len {
        return Err("event-stream headers length exceeds frame".to_string());
    }
    let payload_start = 12 + headers_len;
    let payload_end = total_len - 4;
    let headers_bytes = &buf[12..12 + headers_len];
    let payload_bytes = &buf[payload_start..payload_end];

    let headers = parse_headers(headers_bytes)?;
    let msg = EventStreamMessage {
        headers,
        payload: payload_bytes.to_vec(),
    };
    use bytes::Buf;
    buf.advance(total_len);
    Ok(Some(msg))
}

fn parse_headers(mut data: &[u8]) -> Result<Vec<(String, EventHeaderValue)>, String> {
    let mut out = Vec::new();
    while !data.is_empty() {
        if data.is_empty() {
            break;
        }
        let name_len = data[0] as usize;
        if data.len() < 1 + name_len + 1 {
            return Err("event-stream header truncated".to_string());
        }
        let name = std::str::from_utf8(&data[1..1 + name_len])
            .map_err(|e| format!("header name not utf8: {}", e))?
            .to_string();
        let header_type = data[1 + name_len];
        let mut cursor = 2 + name_len;
        let value = match header_type {
            7 => {
                if data.len() < cursor + 2 {
                    return Err("event-stream header value length truncated".to_string());
                }
                let value_len = u16::from_be_bytes([data[cursor], data[cursor + 1]]) as usize;
                cursor += 2;
                if data.len() < cursor + value_len {
                    return Err("event-stream header value truncated".to_string());
                }
                let value = std::str::from_utf8(&data[cursor..cursor + value_len])
                    .map_err(|e| format!("header value not utf8: {}", e))?
                    .to_string();
                cursor += value_len;
                EventHeaderValue::String(value)
            }
            _ => {
                // We do not need other header types for Bedrock chunk decoding.
                // Skip the value by best-effort length; if unknown, abort.
                return Err(format!("unsupported event-stream header type {}", header_type));
            }
        };
        out.push((name, value));
        data = &data[cursor..];
    }
    Ok(out)
}
