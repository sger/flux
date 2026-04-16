const INDENT: &str = "    ";

pub fn format_source(source: &str) -> String {
    let mut out = String::new();
    let mut indent = 0usize;
    let mut first = true;

    for line in source.lines() {
        let trimmed = line.trim();
        if trimmed.is_empty() {
            out.push('\n');
            continue;
        }

        let leading_closes = leading_close_count(trimmed);
        indent = indent.saturating_sub(leading_closes);

        if !first && !out.ends_with('\n') {
            out.push('\n');
        }
        first = false;

        out.push_str(&INDENT.repeat(indent));
        out.push_str(trimmed);
        out.push('\n');

        indent = indent.saturating_add(brace_delta(trimmed, leading_closes));
    }

    // Trim trailing newlines to avoid extra blank lines at EOF.
    while out.ends_with('\n') {
        out.pop();
    }

    out
}

fn leading_close_count(line: &str) -> usize {
    let mut count = 0;
    let mut chars = line.chars().peekable();
    while let Some(&ch) = chars.peek() {
        if ch.is_whitespace() {
            chars.next();
            continue;
        }
        if ch == '}' || ch == ')' || ch == ']' {
            count += 1;
            chars.next();
            continue;
        }
        // Handle array close delimiter: |]
        if ch == '|' {
            chars.next();
            if chars.peek() == Some(&']') {
                count += 1;
                chars.next();
                continue;
            }
        }
        break;
    }
    count
}

fn brace_delta(line: &str, leading_closes: usize) -> usize {
    let mut opens: i32 = 0;
    let mut closes: i32 = 0;
    let mut in_string = false;
    let mut chars = line.chars().peekable();
    while let Some(ch) = chars.next() {
        if !in_string && ch == '/' && chars.peek() == Some(&'/') {
            break;
        }
        if ch == '"' {
            in_string = !in_string;
            continue;
        }
        if in_string {
            if ch == '\\' {
                chars.next();
            }
            continue;
        }
        match ch {
            '{' | '(' | '[' => opens += 1,
            '}' | ')' | ']' => closes += 1,
            _ => {}
        }
    }
    // Leading `}` are already subtracted from indent before this call, so
    // exclude them from the closes count to avoid double-counting.
    // e.g. `} else {`: opens=1, closes=1, leading_closes=1 → delta=1 (correct)
    let delta = opens - (closes - leading_closes as i32);
    delta.max(0) as usize
}
