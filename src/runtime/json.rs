use std::collections::BTreeMap;

#[derive(Debug, Clone, PartialEq)]
pub enum JsonValue {
    Null,
    Bool(bool),
    Number(JsonNumber),
    String(String),
    Array(Vec<JsonValue>),
    Object(BTreeMap<String, JsonValue>),
}

#[derive(Debug, Clone, PartialEq)]
pub enum JsonNumber {
    Int(i64),
    Float(f64),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct JsonError {
    pub path: String,
    pub message: String,
}

impl JsonError {
    fn new(path: impl Into<String>, message: impl Into<String>) -> Self {
        Self {
            path: path.into(),
            message: message.into(),
        }
    }
}

pub fn parse(input: &str) -> Result<JsonValue, JsonError> {
    let mut parser = Parser {
        input: input.as_bytes(),
        pos: 0,
    };
    let value = parser.parse_value("$")?;
    parser.skip_ws();
    if parser.pos != parser.input.len() {
        return Err(JsonError::new("$", "trailing characters after JSON value"));
    }
    Ok(value)
}

pub fn stringify(value: &JsonValue) -> String {
    let mut out = String::new();
    write_json(value, &mut out);
    out
}

struct Parser<'a> {
    input: &'a [u8],
    pos: usize,
}

impl Parser<'_> {
    fn parse_value(&mut self, path: &str) -> Result<JsonValue, JsonError> {
        self.skip_ws();
        match self.peek() {
            Some(b'n') => {
                self.expect_literal(b"null", path)?;
                Ok(JsonValue::Null)
            }
            Some(b't') => {
                self.expect_literal(b"true", path)?;
                Ok(JsonValue::Bool(true))
            }
            Some(b'f') => {
                self.expect_literal(b"false", path)?;
                Ok(JsonValue::Bool(false))
            }
            Some(b'"') => self.parse_string(path).map(JsonValue::String),
            Some(b'[') => self.parse_array(path),
            Some(b'{') => self.parse_object(path),
            Some(b'-' | b'0'..=b'9') => self.parse_number(path).map(JsonValue::Number),
            Some(_) => Err(JsonError::new(path, "unexpected character in JSON value")),
            None => Err(JsonError::new(path, "unexpected end of input")),
        }
    }

    fn parse_array(&mut self, path: &str) -> Result<JsonValue, JsonError> {
        self.pos += 1;
        self.skip_ws();
        let mut values = Vec::new();
        if self.consume(b']') {
            return Ok(JsonValue::Array(values));
        }
        loop {
            let item_path = format!("{path}[{}]", values.len());
            values.push(self.parse_value(&item_path)?);
            self.skip_ws();
            if self.consume(b']') {
                break;
            }
            if !self.consume(b',') {
                return Err(JsonError::new(path, "expected ',' or ']' in array"));
            }
        }
        Ok(JsonValue::Array(values))
    }

    fn parse_object(&mut self, path: &str) -> Result<JsonValue, JsonError> {
        self.pos += 1;
        self.skip_ws();
        let mut map = BTreeMap::new();
        if self.consume(b'}') {
            return Ok(JsonValue::Object(map));
        }
        loop {
            self.skip_ws();
            if self.peek() != Some(b'"') {
                return Err(JsonError::new(path, "expected string object key"));
            }
            let key = self.parse_string(path)?;
            self.skip_ws();
            if !self.consume(b':') {
                return Err(JsonError::new(path, "expected ':' after object key"));
            }
            let value_path = object_path(path, &key);
            let value = self.parse_value(&value_path)?;
            map.insert(key, value);
            self.skip_ws();
            if self.consume(b'}') {
                break;
            }
            if !self.consume(b',') {
                return Err(JsonError::new(path, "expected ',' or '}' in object"));
            }
        }
        Ok(JsonValue::Object(map))
    }

    fn parse_string(&mut self, path: &str) -> Result<String, JsonError> {
        if !self.consume(b'"') {
            return Err(JsonError::new(path, "expected string"));
        }
        let mut out = String::new();
        while let Some(ch) = self.next() {
            match ch {
                b'"' => return Ok(out),
                b'\\' => self.parse_escape(path, &mut out)?,
                0x00..=0x1f => {
                    return Err(JsonError::new(path, "control character in string"));
                }
                _ => {
                    let start = self.pos - 1;
                    while let Some(b) = self.peek() {
                        if b == b'"' || b == b'\\' || b <= 0x1f {
                            break;
                        }
                        self.pos += 1;
                    }
                    let slice = &self.input[start..self.pos];
                    let text = std::str::from_utf8(slice)
                        .map_err(|_| JsonError::new(path, "invalid UTF-8 in string"))?;
                    out.push_str(text);
                }
            }
        }
        Err(JsonError::new(path, "unterminated string"))
    }

    fn parse_escape(&mut self, path: &str, out: &mut String) -> Result<(), JsonError> {
        match self.next() {
            Some(b'"') => out.push('"'),
            Some(b'\\') => out.push('\\'),
            Some(b'/') => out.push('/'),
            Some(b'b') => out.push('\u{0008}'),
            Some(b'f') => out.push('\u{000c}'),
            Some(b'n') => out.push('\n'),
            Some(b'r') => out.push('\r'),
            Some(b't') => out.push('\t'),
            Some(b'u') => {
                let cp = self.parse_hex4(path)?;
                if (0xd800..=0xdbff).contains(&cp) {
                    if self.next() != Some(b'\\') || self.next() != Some(b'u') {
                        return Err(JsonError::new(path, "missing low surrogate"));
                    }
                    let low = self.parse_hex4(path)?;
                    if !(0xdc00..=0xdfff).contains(&low) {
                        return Err(JsonError::new(path, "invalid low surrogate"));
                    }
                    let scalar = 0x10000 + (((cp - 0xd800) << 10) | (low - 0xdc00));
                    let ch = char::from_u32(scalar)
                        .ok_or_else(|| JsonError::new(path, "invalid unicode escape"))?;
                    out.push(ch);
                } else if (0xdc00..=0xdfff).contains(&cp) {
                    return Err(JsonError::new(path, "unexpected low surrogate"));
                } else {
                    let ch = char::from_u32(cp)
                        .ok_or_else(|| JsonError::new(path, "invalid unicode escape"))?;
                    out.push(ch);
                }
            }
            Some(_) => return Err(JsonError::new(path, "invalid string escape")),
            None => return Err(JsonError::new(path, "unterminated string escape")),
        }
        Ok(())
    }

    fn parse_hex4(&mut self, path: &str) -> Result<u32, JsonError> {
        if self.pos + 4 > self.input.len() {
            return Err(JsonError::new(path, "incomplete unicode escape"));
        }
        let mut value = 0u32;
        for _ in 0..4 {
            let digit = self.next().unwrap();
            value = value * 16
                + match digit {
                    b'0'..=b'9' => (digit - b'0') as u32,
                    b'a'..=b'f' => (digit - b'a' + 10) as u32,
                    b'A'..=b'F' => (digit - b'A' + 10) as u32,
                    _ => return Err(JsonError::new(path, "invalid unicode escape")),
                };
        }
        Ok(value)
    }

    fn parse_number(&mut self, path: &str) -> Result<JsonNumber, JsonError> {
        let start = self.pos;
        self.consume(b'-');
        match self.peek() {
            Some(b'0') => {
                self.pos += 1;
                if matches!(self.peek(), Some(b'0'..=b'9')) {
                    return Err(JsonError::new(path, "invalid number"));
                }
            }
            Some(b'1'..=b'9') => {
                self.pos += 1;
                while matches!(self.peek(), Some(b'0'..=b'9')) {
                    self.pos += 1;
                }
            }
            _ => return Err(JsonError::new(path, "invalid number")),
        }
        let mut integral = true;
        if self.consume(b'.') {
            integral = false;
            if !matches!(self.peek(), Some(b'0'..=b'9')) {
                return Err(JsonError::new(path, "invalid number"));
            }
            while matches!(self.peek(), Some(b'0'..=b'9')) {
                self.pos += 1;
            }
        }
        if matches!(self.peek(), Some(b'e' | b'E')) {
            integral = false;
            self.pos += 1;
            if matches!(self.peek(), Some(b'+' | b'-')) {
                self.pos += 1;
            }
            if !matches!(self.peek(), Some(b'0'..=b'9')) {
                return Err(JsonError::new(path, "invalid number"));
            }
            while matches!(self.peek(), Some(b'0'..=b'9')) {
                self.pos += 1;
            }
        }
        let raw = std::str::from_utf8(&self.input[start..self.pos])
            .map_err(|_| JsonError::new(path, "invalid number"))?;
        if integral {
            if let Ok(n) = raw.parse::<i64>() {
                return Ok(JsonNumber::Int(n));
            }
        }
        let n = raw
            .parse::<f64>()
            .map_err(|_| JsonError::new(path, "invalid number"))?;
        if !n.is_finite() {
            return Err(JsonError::new(path, "number is out of range"));
        }
        Ok(JsonNumber::Float(n))
    }

    fn expect_literal(&mut self, literal: &[u8], path: &str) -> Result<(), JsonError> {
        if self.input.get(self.pos..self.pos + literal.len()) == Some(literal) {
            self.pos += literal.len();
            Ok(())
        } else {
            Err(JsonError::new(path, "invalid literal"))
        }
    }

    fn skip_ws(&mut self) {
        while matches!(self.peek(), Some(b' ' | b'\n' | b'\r' | b'\t')) {
            self.pos += 1;
        }
    }

    fn consume(&mut self, byte: u8) -> bool {
        if self.peek() == Some(byte) {
            self.pos += 1;
            true
        } else {
            false
        }
    }

    fn next(&mut self) -> Option<u8> {
        let ch = self.peek()?;
        self.pos += 1;
        Some(ch)
    }

    fn peek(&self) -> Option<u8> {
        self.input.get(self.pos).copied()
    }
}

fn object_path(parent: &str, key: &str) -> String {
    if key.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'_') {
        format!("{parent}.{key}")
    } else {
        format!(
            "{parent}[{}]",
            stringify(&JsonValue::String(key.to_string()))
        )
    }
}

fn write_json(value: &JsonValue, out: &mut String) {
    match value {
        JsonValue::Null => out.push_str("null"),
        JsonValue::Bool(true) => out.push_str("true"),
        JsonValue::Bool(false) => out.push_str("false"),
        JsonValue::Number(JsonNumber::Int(n)) => out.push_str(&n.to_string()),
        JsonValue::Number(JsonNumber::Float(n)) => out.push_str(&format_number(*n)),
        JsonValue::String(s) => write_string(s, out),
        JsonValue::Array(values) => {
            out.push('[');
            for (idx, item) in values.iter().enumerate() {
                if idx > 0 {
                    out.push(',');
                }
                write_json(item, out);
            }
            out.push(']');
        }
        JsonValue::Object(map) => {
            out.push('{');
            for (idx, (key, item)) in map.iter().enumerate() {
                if idx > 0 {
                    out.push(',');
                }
                write_string(key, out);
                out.push(':');
                write_json(item, out);
            }
            out.push('}');
        }
    }
}

fn write_string(s: &str, out: &mut String) {
    out.push('"');
    for ch in s.chars() {
        match ch {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\u{0008}' => out.push_str("\\b"),
            '\u{000c}' => out.push_str("\\f"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            ch if ch <= '\u{001f}' => out.push_str(&format!("\\u{:04x}", ch as u32)),
            ch => out.push(ch),
        }
    }
    out.push('"');
}

fn format_number(n: f64) -> String {
    if n.fract() == 0.0 && n.abs() < 1e21 {
        format!("{n:.0}")
    } else {
        format!("{n}")
    }
}

#[cfg(test)]
mod tests {
    use super::{JsonNumber, JsonValue, parse, stringify};

    #[test]
    fn integer_numbers_parse_and_stringify_losslessly() {
        let value = parse("9007199254740993").expect("parse large integer");
        assert_eq!(
            value,
            JsonValue::Number(JsonNumber::Int(9_007_199_254_740_993))
        );
        assert_eq!(stringify(&value), "9007199254740993");
    }

    #[test]
    fn fractional_and_exponent_numbers_remain_float_numbers() {
        assert_eq!(
            parse("3.5").expect("parse fraction"),
            JsonValue::Number(JsonNumber::Float(3.5))
        );
        assert_eq!(
            parse("1e3").expect("parse exponent"),
            JsonValue::Number(JsonNumber::Float(1000.0))
        );
    }

    #[test]
    fn all_json_variants_stringify_deterministically() {
        let value = parse(r#"{"b":[true,null,"x"],"a":-7}"#).expect("parse object");
        assert_eq!(stringify(&value), r#"{"a":-7,"b":[true,null,"x"]}"#);
    }
}
