//! Debug Adapter Protocol framing: each message is a `Content-Length`
//! header, a blank line, and that many bytes of JSON.

use std::io::{self, BufRead, Write};

use serde_json::Value;

/// Reads one message. `None` at the end of the stream.
pub fn read_message(input: &mut impl BufRead) -> io::Result<Option<Value>> {
    let mut length = None;
    loop {
        let mut line = String::new();
        if input.read_line(&mut line)? == 0 {
            return Ok(None);
        }
        let line = line.trim_end_matches(['\r', '\n']);
        if line.is_empty() {
            if length.is_some() {
                break;
            }
            continue;
        }
        if let Some((name, value)) = line.split_once(':')
            && name.trim().eq_ignore_ascii_case("content-length")
        {
            length =
                Some(value.trim().parse::<usize>().map_err(|_| {
                    io::Error::new(io::ErrorKind::InvalidData, "bad Content-Length")
                })?);
        }
    }
    let mut body = vec![0; length.unwrap_or(0)];
    input.read_exact(&mut body)?;
    serde_json::from_slice(&body)
        .map(Some)
        .map_err(|err| io::Error::new(io::ErrorKind::InvalidData, err))
}

/// Writes one message.
pub fn write_message(output: &mut impl Write, message: &Value) -> io::Result<()> {
    let body = serde_json::to_vec(message).expect("DAP messages serialize");
    write!(output, "Content-Length: {}\r\n\r\n", body.len())?;
    output.write_all(&body)?;
    output.flush()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn messages_round_trip_through_the_framing() {
        let message = serde_json::json!({"seq": 1, "type": "request", "command": "initialize"});
        let mut bytes = Vec::new();
        write_message(&mut bytes, &message).unwrap();
        write_message(&mut bytes, &message).unwrap();
        let mut input = io::Cursor::new(bytes);
        assert_eq!(read_message(&mut input).unwrap(), Some(message.clone()));
        assert_eq!(read_message(&mut input).unwrap(), Some(message));
        assert_eq!(read_message(&mut input).unwrap(), None);
    }
}
