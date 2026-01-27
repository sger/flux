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

        indent = indent.saturating_add(brace_delta(trimmed));
    }

    // Trim trailing newlines to avoid extra blank lines at EOF.
    while out.ends_with('\n') {
        out.pop();
    }

    out
}

fn leading_close_count(line: &str) -> usize {
    let mut count = 0;
    for ch in line.chars() {
        if ch.is_whitespace() {
            continue;
        }
        if ch == '}' {
            count += 1;
            continue;
        }
        break;
    }
    count
}

fn brace_delta(line: &str) -> usize {
    let mut delta: i32 = 0;
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
            continue;
        }
        match ch {
            '{' => delta += 1,
            '}' => delta -= 1,
            _ => {}
        }
    }
    if delta < 0 {
        0
    } else {
        delta as usize
    }
}
