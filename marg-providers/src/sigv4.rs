use chrono::{DateTime, Utc};
use data_encoding::HEXLOWER;
use hmac::{Hmac, Mac};
use percent_encoding::{utf8_percent_encode, AsciiSet, CONTROLS};
use sha2::{Digest, Sha256};

type HmacSha256 = Hmac<Sha256>;

const UNRESERVED_OVERRIDE: &AsciiSet = &CONTROLS
    .add(b' ')
    .add(b'"')
    .add(b'#')
    .add(b'%')
    .add(b'<')
    .add(b'>')
    .add(b'?')
    .add(b'`')
    .add(b'{')
    .add(b'}')
    .add(b'^')
    .add(b'|')
    .add(b'\\')
    .add(b':')
    .add(b';')
    .add(b'=')
    .add(b'+')
    .add(b'&')
    .add(b',')
    .add(b'/')
    .add(b'@')
    .add(b'!')
    .add(b'$')
    .add(b'(')
    .add(b')')
    .add(b'\'')
    .add(b'*');

pub struct Credentials<'a> {
    pub access_key_id: &'a str,
    pub secret_access_key: &'a str,
    pub session_token: Option<&'a str>,
}

pub struct SignedRequest {
    pub authorization: String,
    pub amz_date: String,
    pub content_sha256: String,
    pub session_token: Option<String>,
}

pub fn sign(
    method: &str,
    host: &str,
    path: &str,
    region: &str,
    service: &str,
    creds: &Credentials<'_>,
    extra_headers: &[(String, String)],
    body: &[u8],
    now: DateTime<Utc>,
) -> SignedRequest {
    let amz_date = now.format("%Y%m%dT%H%M%SZ").to_string();
    let date_stamp = now.format("%Y%m%d").to_string();

    let payload_hash = HEXLOWER.encode(&Sha256::digest(body));

    let mut headers: Vec<(String, String)> = vec![
        ("host".to_string(), host.to_string()),
        ("x-amz-content-sha256".to_string(), payload_hash.clone()),
        ("x-amz-date".to_string(), amz_date.clone()),
    ];
    if let Some(tok) = creds.session_token {
        headers.push(("x-amz-security-token".to_string(), tok.to_string()));
    }
    for (k, v) in extra_headers {
        headers.push((k.to_ascii_lowercase(), v.trim().to_string()));
    }
    headers.sort_by(|a, b| a.0.cmp(&b.0));

    let canonical_headers: String = headers
        .iter()
        .map(|(k, v)| format!("{}:{}\n", k, v))
        .collect();
    let signed_headers: String = headers
        .iter()
        .map(|(k, _)| k.as_str())
        .collect::<Vec<_>>()
        .join(";");

    let canonical_uri = canonicalize_uri(path);
    let canonical_query = ""; // Bedrock endpoints used here have no query string.
    let canonical_request = format!(
        "{}\n{}\n{}\n{}\n{}\n{}",
        method, canonical_uri, canonical_query, canonical_headers, signed_headers, payload_hash
    );
    let canonical_request_hash = HEXLOWER.encode(&Sha256::digest(canonical_request.as_bytes()));
    let credential_scope = format!("{}/{}/{}/aws4_request", date_stamp, region, service);
    let string_to_sign = format!(
        "AWS4-HMAC-SHA256\n{}\n{}\n{}",
        amz_date, credential_scope, canonical_request_hash
    );

    let k_date = hmac(format!("AWS4{}", creds.secret_access_key).as_bytes(), date_stamp.as_bytes());
    let k_region = hmac(&k_date, region.as_bytes());
    let k_service = hmac(&k_region, service.as_bytes());
    let k_signing = hmac(&k_service, b"aws4_request");
    let signature = HEXLOWER.encode(&hmac(&k_signing, string_to_sign.as_bytes()));

    let authorization = format!(
        "AWS4-HMAC-SHA256 Credential={}/{},SignedHeaders={},Signature={}",
        creds.access_key_id, credential_scope, signed_headers, signature
    );

    SignedRequest {
        authorization,
        amz_date,
        content_sha256: payload_hash,
        session_token: creds.session_token.map(|s| s.to_string()),
    }
}

fn hmac(key: &[u8], msg: &[u8]) -> Vec<u8> {
    let mut mac = HmacSha256::new_from_slice(key).expect("HMAC accepts any key length");
    mac.update(msg);
    mac.finalize().into_bytes().to_vec()
}

fn canonicalize_uri(path: &str) -> String {
    if path.is_empty() {
        return "/".to_string();
    }
    let segments: Vec<&str> = path.split('/').collect();
    let encoded: Vec<String> = segments
        .iter()
        .map(|seg| utf8_percent_encode(seg, UNRESERVED_OVERRIDE).to_string())
        .collect();
    encoded.join("/")
}
