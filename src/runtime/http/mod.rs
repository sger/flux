//! HTTP/1.1 parser, writer, and blocking VM server foundation.
//!
//! This module owns HTTP wire behavior only. The blocking server helper accepts
//! a Rust callback for each parsed request; VM value construction and Flux
//! handler invocation stay in `src/vm/core_dispatch.rs`.

use std::collections::BTreeMap;
use std::io::{Read, Write};
use std::net::{TcpListener, TcpStream};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HttpRequest {
    pub method: String,
    pub target: String,
    pub headers: Vec<(String, String)>,
    pub body: Vec<u8>,
    pub keep_alive: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HttpResponse {
    pub status: u16,
    pub reason: String,
    pub headers: Vec<(String, String)>,
    pub body: Vec<u8>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HttpUrl {
    pub host: String,
    pub port: u16,
    pub target: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum HttpError {
    NeedMore,
    BadRequest(String),
    PayloadTooLarge(String),
}

#[derive(Debug, Clone, Copy)]
pub struct ParseLimits {
    pub max_header_bytes: usize,
    pub max_body_bytes: usize,
}

impl Default for ParseLimits {
    fn default() -> Self {
        Self {
            max_header_bytes: 65_536,
            max_body_bytes: 8_388_608,
        }
    }
}

#[derive(Debug, Clone, Copy)]
pub struct BlockingServerConfig {
    pub max_connections: usize,
    pub limits: ParseLimits,
    pub request_timeout_ms: usize,
    pub worker_count: Option<usize>,
}

impl Default for BlockingServerConfig {
    fn default() -> Self {
        Self {
            max_connections: 10_000,
            limits: ParseLimits::default(),
            request_timeout_ms: 30_000,
            worker_count: None,
        }
    }
}

pub fn serve_blocking<F>(
    host: &str,
    port: u16,
    config: BlockingServerConfig,
    mut handler: F,
) -> Result<usize, String>
where
    F: FnMut(HttpRequest) -> Result<HttpResponse, String>,
{
    if config.max_connections == 0 {
        return Ok(0);
    }

    let listener = TcpListener::bind((host, port))
        .map_err(|e| format!("http_serve_config: bind failed: {e}"))?;
    let mut accepted = 0usize;
    while accepted < config.max_connections {
        let (mut stream, _) = listener
            .accept()
            .map_err(|e| format!("http_serve_config: accept failed: {e}"))?;
        accepted += 1;
        handle_connection(&mut stream, config.limits, &mut handler)?;
    }
    Ok(accepted)
}

fn handle_connection<F>(
    stream: &mut TcpStream,
    limits: ParseLimits,
    handler: &mut F,
) -> Result<(), String>
where
    F: FnMut(HttpRequest) -> Result<HttpResponse, String>,
{
    let mut buf = Vec::new();
    let mut chunk = [0u8; 4096];

    loop {
        let (req, used) = loop {
            match parse_request(&buf, limits) {
                Ok(parsed) => break parsed,
                Err(HttpError::NeedMore) => {
                    let n = stream
                        .read(&mut chunk)
                        .map_err(|e| format!("http_serve_config: read failed: {e}"))?;
                    if n == 0 {
                        return Ok(());
                    }
                    buf.extend_from_slice(&chunk[..n]);
                }
                Err(HttpError::PayloadTooLarge(msg)) => {
                    write_error_response(stream, 413, "Payload Too Large", msg)?;
                    return Ok(());
                }
                Err(HttpError::BadRequest(msg)) => {
                    write_error_response(stream, 400, "Bad Request", msg)?;
                    return Ok(());
                }
            }
        };

        buf.drain(..used);
        let keep_alive = req.keep_alive;
        let mut response = handler(req)?;
        ensure_connection_header(&mut response, keep_alive);
        let wire = write_response(&response);
        stream
            .write_all(&wire)
            .map_err(|e| format!("http_serve_config: write failed: {e}"))?;
        if !keep_alive {
            return Ok(());
        }
    }
}

fn write_error_response(
    stream: &mut TcpStream,
    status: u16,
    reason: &str,
    message: String,
) -> Result<(), String> {
    let response = HttpResponse {
        status,
        reason: reason.into(),
        headers: vec![("Connection".into(), "close".into())],
        body: message.into_bytes(),
    };
    let wire = write_response(&response);
    stream
        .write_all(&wire)
        .map_err(|e| format!("http_serve_config: write failed: {e}"))
}

fn ensure_connection_header(response: &mut HttpResponse, keep_alive: bool) {
    if response
        .headers
        .iter()
        .any(|(name, _)| name.eq_ignore_ascii_case("connection"))
    {
        return;
    }
    response.headers.push((
        "Connection".into(),
        if keep_alive { "keep-alive" } else { "close" }.into(),
    ));
}

pub fn parse_request(input: &[u8], limits: ParseLimits) -> Result<(HttpRequest, usize), HttpError> {
    let header_end = find_header_end(input).ok_or_else(|| {
        if input.len() > limits.max_header_bytes {
            HttpError::PayloadTooLarge("HTTP header block exceeds max_header_bytes".into())
        } else {
            HttpError::NeedMore
        }
    })?;
    if header_end > limits.max_header_bytes {
        return Err(HttpError::PayloadTooLarge(
            "HTTP header block exceeds max_header_bytes".into(),
        ));
    }

    let head = std::str::from_utf8(&input[..header_end])
        .map_err(|_| HttpError::BadRequest("HTTP header block is not valid UTF-8".into()))?;
    let mut lines = head.split("\r\n");
    let request_line = lines
        .next()
        .ok_or_else(|| HttpError::BadRequest("missing request line".into()))?;
    let (method, target, version) = parse_request_line(request_line)?;
    if version != "HTTP/1.1" {
        return Err(HttpError::BadRequest(format!(
            "unsupported HTTP version {version}"
        )));
    }

    let mut headers = Vec::new();
    let mut normalized: BTreeMap<String, Vec<String>> = BTreeMap::new();
    for line in lines {
        if line.is_empty() {
            continue;
        }
        if line.starts_with(' ') || line.starts_with('\t') {
            return Err(HttpError::BadRequest(
                "obsolete folded HTTP headers are rejected".into(),
            ));
        }
        let (name, value) = parse_header_line(line)?;
        normalized
            .entry(name.to_ascii_lowercase())
            .or_default()
            .push(value.clone());
        headers.push((name, value));
    }

    let content_lengths = normalized.get("content-length");
    let transfer_encoding = normalized
        .get("transfer-encoding")
        .and_then(|values| values.last())
        .map(|v| v.to_ascii_lowercase());

    let body_start = header_end + 4;
    let (body, consumed) = match (content_lengths, transfer_encoding.as_deref()) {
        (Some(_), Some(te)) if te.contains("chunked") => {
            return Err(HttpError::BadRequest(
                "conflicting Content-Length and Transfer-Encoding".into(),
            ));
        }
        (_, Some(te)) if te.contains("chunked") => decode_chunked(&input[body_start..], limits)?,
        (Some(values), _) => {
            let len = parse_content_length(values)?;
            if len > limits.max_body_bytes {
                return Err(HttpError::PayloadTooLarge(
                    "HTTP body exceeds max_body_bytes".into(),
                ));
            }
            if input.len() < body_start + len {
                return Err(HttpError::NeedMore);
            }
            (input[body_start..body_start + len].to_vec(), len)
        }
        _ => (Vec::new(), 0),
    };

    let connection = normalized
        .get("connection")
        .and_then(|values| values.last())
        .map(|v| v.to_ascii_lowercase())
        .unwrap_or_default();
    let keep_alive = connection != "close";

    Ok((
        HttpRequest {
            method: method.to_string(),
            target: target.to_string(),
            headers,
            body,
            keep_alive,
        },
        body_start + consumed,
    ))
}

pub fn write_response(resp: &HttpResponse) -> Vec<u8> {
    let mut out = Vec::new();
    out.extend_from_slice(format!("HTTP/1.1 {} {}\r\n", resp.status, resp.reason).as_bytes());
    let mut has_content_length = false;
    for (name, value) in &resp.headers {
        if name.eq_ignore_ascii_case("content-length") {
            has_content_length = true;
        }
        out.extend_from_slice(name.as_bytes());
        out.extend_from_slice(b": ");
        out.extend_from_slice(value.as_bytes());
        out.extend_from_slice(b"\r\n");
    }
    if !has_content_length {
        out.extend_from_slice(format!("Content-Length: {}\r\n", resp.body.len()).as_bytes());
    }
    out.extend_from_slice(b"\r\n");
    out.extend_from_slice(&resp.body);
    out
}

pub fn write_chunked_head(status: u16, reason: &str, headers: &[(String, String)]) -> Vec<u8> {
    let mut out = Vec::new();
    out.extend_from_slice(format!("HTTP/1.1 {status} {reason}\r\n").as_bytes());
    for (name, value) in headers {
        if name.eq_ignore_ascii_case("content-length")
            || name.eq_ignore_ascii_case("transfer-encoding")
            || name.eq_ignore_ascii_case("connection")
        {
            continue;
        }
        out.extend_from_slice(name.as_bytes());
        out.extend_from_slice(b": ");
        out.extend_from_slice(value.as_bytes());
        out.extend_from_slice(b"\r\n");
    }
    out.extend_from_slice(b"Transfer-Encoding: chunked\r\n");
    out.extend_from_slice(b"Connection: close\r\n");
    out.extend_from_slice(b"\r\n");
    out
}

pub fn write_chunk(chunk: &[u8]) -> Vec<u8> {
    let mut out = Vec::new();
    out.extend_from_slice(format!("{:X}\r\n", chunk.len()).as_bytes());
    out.extend_from_slice(chunk);
    out.extend_from_slice(b"\r\n");
    out
}

pub fn write_chunked_end() -> Vec<u8> {
    b"0\r\n\r\n".to_vec()
}

pub fn parse_url(url: &str) -> Result<HttpUrl, HttpError> {
    let Some(rest) = url.strip_prefix("http://") else {
        return Err(HttpError::BadRequest(
            "only http:// URLs are supported in this phase".into(),
        ));
    };
    let (authority, target) = match rest.find(['/', '?']) {
        Some(idx) => {
            let path = &rest[idx..];
            let target = if path.starts_with('?') {
                format!("/{path}")
            } else {
                path.to_string()
            };
            (&rest[..idx], target)
        }
        None => (rest, "/".to_string()),
    };
    if authority.is_empty() {
        return Err(HttpError::BadRequest("HTTP URL missing host".into()));
    }
    let (host, port) = if let Some((host, port_raw)) = authority.rsplit_once(':') {
        if host.is_empty() {
            return Err(HttpError::BadRequest("HTTP URL missing host".into()));
        }
        let port = port_raw
            .parse::<u16>()
            .map_err(|_| HttpError::BadRequest("HTTP URL has invalid port".into()))?;
        (host, port)
    } else {
        (authority, 80)
    };
    Ok(HttpUrl {
        host: host.to_string(),
        port,
        target: if target.is_empty() {
            "/".into()
        } else {
            target
        },
    })
}

pub fn write_request(
    method: &str,
    host: &str,
    target: &str,
    headers: &[(String, String)],
    body: &[u8],
) -> Vec<u8> {
    let mut out = Vec::new();
    out.extend_from_slice(format!("{method} {target} HTTP/1.1\r\n").as_bytes());
    let mut has_host = false;
    let mut has_connection = false;
    let mut has_content_length = false;
    for (name, value) in headers {
        if name.eq_ignore_ascii_case("host") {
            has_host = true;
        } else if name.eq_ignore_ascii_case("connection") {
            has_connection = true;
        } else if name.eq_ignore_ascii_case("content-length") {
            has_content_length = true;
        }
        out.extend_from_slice(name.as_bytes());
        out.extend_from_slice(b": ");
        out.extend_from_slice(value.as_bytes());
        out.extend_from_slice(b"\r\n");
    }
    if !has_host {
        out.extend_from_slice(format!("Host: {host}\r\n").as_bytes());
    }
    if !has_connection {
        out.extend_from_slice(b"Connection: close\r\n");
    }
    if !has_content_length {
        out.extend_from_slice(format!("Content-Length: {}\r\n", body.len()).as_bytes());
    }
    out.extend_from_slice(b"\r\n");
    out.extend_from_slice(body);
    out
}

pub fn parse_response(
    input: &[u8],
    limits: ParseLimits,
) -> Result<(HttpResponse, usize), HttpError> {
    let header_end = find_header_end(input).ok_or_else(|| {
        if input.len() > limits.max_header_bytes {
            HttpError::PayloadTooLarge("HTTP response header block exceeds max_header_bytes".into())
        } else {
            HttpError::NeedMore
        }
    })?;
    if header_end > limits.max_header_bytes {
        return Err(HttpError::PayloadTooLarge(
            "HTTP response header block exceeds max_header_bytes".into(),
        ));
    }
    let head = std::str::from_utf8(&input[..header_end]).map_err(|_| {
        HttpError::BadRequest("HTTP response header block is not valid UTF-8".into())
    })?;
    let mut lines = head.split("\r\n");
    let status_line = lines
        .next()
        .ok_or_else(|| HttpError::BadRequest("missing status line".into()))?;
    let (status, reason) = parse_status_line(status_line)?;
    let mut headers = Vec::new();
    let mut normalized: BTreeMap<String, Vec<String>> = BTreeMap::new();
    for line in lines {
        if line.is_empty() {
            continue;
        }
        if line.starts_with(' ') || line.starts_with('\t') {
            return Err(HttpError::BadRequest(
                "obsolete folded HTTP headers are rejected".into(),
            ));
        }
        let (name, value) = parse_header_line(line)?;
        normalized
            .entry(name.to_ascii_lowercase())
            .or_default()
            .push(value.clone());
        headers.push((name, value));
    }
    let content_lengths = normalized.get("content-length");
    let transfer_encoding = normalized
        .get("transfer-encoding")
        .and_then(|values| values.last())
        .map(|v| v.to_ascii_lowercase());
    let body_start = header_end + 4;
    let (body, consumed) = match (content_lengths, transfer_encoding.as_deref()) {
        (Some(_), Some(te)) if te.contains("chunked") => {
            return Err(HttpError::BadRequest(
                "conflicting Content-Length and Transfer-Encoding".into(),
            ));
        }
        (_, Some(te)) if te.contains("chunked") => decode_chunked(&input[body_start..], limits)?,
        (Some(values), _) => {
            let len = parse_content_length(values)?;
            if len > limits.max_body_bytes {
                return Err(HttpError::PayloadTooLarge(
                    "HTTP response body exceeds max_body_bytes".into(),
                ));
            }
            if input.len() < body_start + len {
                return Err(HttpError::NeedMore);
            }
            (input[body_start..body_start + len].to_vec(), len)
        }
        _ => (Vec::new(), 0),
    };
    Ok((
        HttpResponse {
            status,
            reason: reason.to_string(),
            headers,
            body,
        },
        body_start + consumed,
    ))
}

fn parse_status_line(line: &str) -> Result<(u16, &str), HttpError> {
    let Some(rest) = line.strip_prefix("HTTP/1.1 ") else {
        return Err(HttpError::BadRequest(
            "unsupported HTTP response version".into(),
        ));
    };
    let (status_raw, reason) = rest.split_once(' ').unwrap_or((rest, ""));
    let status = status_raw
        .parse::<u16>()
        .map_err(|_| HttpError::BadRequest("invalid HTTP response status".into()))?;
    Ok((status, reason))
}

fn parse_request_line(line: &str) -> Result<(&str, &str, &str), HttpError> {
    let mut parts = line.split_ascii_whitespace();
    let method = parts
        .next()
        .ok_or_else(|| HttpError::BadRequest("missing HTTP method".into()))?;
    let target = parts
        .next()
        .ok_or_else(|| HttpError::BadRequest("missing request target".into()))?;
    let version = parts
        .next()
        .ok_or_else(|| HttpError::BadRequest("missing HTTP version".into()))?;
    if parts.next().is_some() {
        return Err(HttpError::BadRequest("too many request-line fields".into()));
    }
    if !method.bytes().all(|b| b.is_ascii_uppercase()) {
        return Err(HttpError::BadRequest("invalid HTTP method token".into()));
    }
    Ok((method, target, version))
}

fn parse_header_line(line: &str) -> Result<(String, String), HttpError> {
    let Some((name, value)) = line.split_once(':') else {
        return Err(HttpError::BadRequest("HTTP header missing ':'".into()));
    };
    if name.is_empty()
        || !name.bytes().all(|b| {
            b.is_ascii_alphanumeric()
                || matches!(
                    b,
                    b'!' | b'#'
                        | b'$'
                        | b'%'
                        | b'&'
                        | b'\''
                        | b'*'
                        | b'+'
                        | b'-'
                        | b'.'
                        | b'^'
                        | b'_'
                        | b'`'
                        | b'|'
                        | b'~'
                )
        })
    {
        return Err(HttpError::BadRequest("invalid HTTP header name".into()));
    }
    Ok((
        name.to_string(),
        value.trim_matches([' ', '\t']).to_string(),
    ))
}

fn parse_content_length(values: &[String]) -> Result<usize, HttpError> {
    let first = values
        .first()
        .ok_or_else(|| HttpError::BadRequest("empty Content-Length".into()))?;
    if values.iter().any(|v| v != first) {
        return Err(HttpError::BadRequest(
            "conflicting Content-Length headers".into(),
        ));
    }
    first
        .parse::<usize>()
        .map_err(|_| HttpError::BadRequest("invalid Content-Length".into()))
}

fn decode_chunked(input: &[u8], limits: ParseLimits) -> Result<(Vec<u8>, usize), HttpError> {
    let mut pos = 0;
    let mut body = Vec::new();
    loop {
        let Some(line_end) = find_crlf(&input[pos..]) else {
            return Err(HttpError::NeedMore);
        };
        let size_line = std::str::from_utf8(&input[pos..pos + line_end])
            .map_err(|_| HttpError::BadRequest("invalid chunk size".into()))?;
        let size_token = size_line.split(';').next().unwrap_or("").trim();
        let size = usize::from_str_radix(size_token, 16)
            .map_err(|_| HttpError::BadRequest("invalid chunk size".into()))?;
        pos += line_end + 2;
        if size == 0 {
            if input.len() < pos + 2 {
                return Err(HttpError::NeedMore);
            }
            if &input[pos..pos + 2] != b"\r\n" {
                return Err(HttpError::BadRequest(
                    "chunk trailer fields are not supported in Phase 3a".into(),
                ));
            }
            return Ok((body, pos + 2));
        }
        if body.len() + size > limits.max_body_bytes {
            return Err(HttpError::PayloadTooLarge(
                "HTTP chunked body exceeds max_body_bytes".into(),
            ));
        }
        if input.len() < pos + size + 2 {
            return Err(HttpError::NeedMore);
        }
        body.extend_from_slice(&input[pos..pos + size]);
        pos += size;
        if &input[pos..pos + 2] != b"\r\n" {
            return Err(HttpError::BadRequest("chunk missing trailing CRLF".into()));
        }
        pos += 2;
    }
}

fn find_header_end(input: &[u8]) -> Option<usize> {
    input.windows(4).position(|w| w == b"\r\n\r\n")
}

fn find_crlf(input: &[u8]) -> Option<usize> {
    input.windows(2).position(|w| w == b"\r\n")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_basic_request() {
        let raw = b"GET /hello HTTP/1.1\r\nHost: example.test\r\n\r\n";
        let (req, used) = parse_request(raw, ParseLimits::default()).unwrap();
        assert_eq!(used, raw.len());
        assert_eq!(req.method, "GET");
        assert_eq!(req.target, "/hello");
        assert_eq!(req.headers[0], ("Host".into(), "example.test".into()));
        assert!(req.keep_alive);
    }

    #[test]
    fn parses_pipelined_requests_by_consumed_offset() {
        let raw = b"GET /one HTTP/1.1\r\nHost: local\r\n\r\nGET /two HTTP/1.1\r\nHost: local\r\nConnection: close\r\n\r\n";
        let (first, used) = parse_request(raw, ParseLimits::default()).unwrap();
        assert_eq!(first.target, "/one");
        assert!(first.keep_alive);

        let (second, second_used) = parse_request(&raw[used..], ParseLimits::default()).unwrap();
        assert_eq!(second.target, "/two");
        assert!(!second.keep_alive);
        assert_eq!(used + second_used, raw.len());
    }

    #[test]
    fn connection_close_marks_request_not_keep_alive() {
        let raw = b"GET / HTTP/1.1\r\nHost: local\r\nConnection: close\r\n\r\n";
        let (req, _) = parse_request(raw, ParseLimits::default()).unwrap();
        assert!(!req.keep_alive);
    }

    #[test]
    fn trims_ows_and_rejects_obs_fold() {
        let raw = b"GET / HTTP/1.1\r\nX-Test:\t value \t\r\n\r\n";
        let (req, _) = parse_request(raw, ParseLimits::default()).unwrap();
        assert_eq!(req.headers[0], ("X-Test".into(), "value".into()));

        let folded = b"GET / HTTP/1.1\r\nX-Test: one\r\n two\r\n\r\n";
        assert!(matches!(
            parse_request(folded, ParseLimits::default()),
            Err(HttpError::BadRequest(_))
        ));
    }

    #[test]
    fn rejects_conflicting_framing_and_content_lengths() {
        let dup = b"POST / HTTP/1.1\r\nContent-Length: 1\r\nContent-Length: 2\r\n\r\nx";
        assert!(matches!(
            parse_request(dup, ParseLimits::default()),
            Err(HttpError::BadRequest(_))
        ));

        let mixed =
            b"POST / HTTP/1.1\r\nContent-Length: 1\r\nTransfer-Encoding: chunked\r\n\r\n0\r\n\r\n";
        assert!(matches!(
            parse_request(mixed, ParseLimits::default()),
            Err(HttpError::BadRequest(_))
        ));
    }

    #[test]
    fn decodes_chunked_body() {
        let raw = b"POST / HTTP/1.1\r\nTransfer-Encoding: chunked\r\n\r\n4\r\nflux\r\n0\r\n\r\n";
        let (req, used) = parse_request(raw, ParseLimits::default()).unwrap();
        assert_eq!(used, raw.len());
        assert_eq!(req.body, b"flux");
    }

    #[test]
    fn enforces_limits() {
        let limits = ParseLimits {
            max_header_bytes: 8,
            max_body_bytes: 4,
        };
        assert!(matches!(
            parse_request(b"GET / HTTP/1.1\r\n\r\n", limits),
            Err(HttpError::PayloadTooLarge(_))
        ));

        let raw = b"POST / HTTP/1.1\r\nContent-Length: 5\r\n\r\nhello";
        assert!(matches!(
            parse_request(raw, limits),
            Err(HttpError::PayloadTooLarge(_))
        ));
    }

    #[test]
    fn writes_content_length_response() {
        let bytes = write_response(&HttpResponse {
            status: 200,
            reason: "OK".into(),
            headers: vec![("Connection".into(), "close".into())],
            body: b"hello".to_vec(),
        });
        let text = String::from_utf8(bytes).unwrap();
        assert!(text.starts_with("HTTP/1.1 200 OK\r\n"));
        assert!(text.contains("Content-Length: 5\r\n"));
        assert!(text.ends_with("\r\n\r\nhello"));
    }
}
