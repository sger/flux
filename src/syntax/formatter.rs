const INDENT: &str = "    ";

pub fn format_source(source: &str) -> String {
    let mut out = String::new();
    let mut indent = 0usize;
    let mut first = true;
    let mut block_comment_depth = 0usize;
    let lines: Vec<&str> = source.lines().collect();

    for (idx, line) in lines.iter().enumerate() {
        let in_block_comment = block_comment_depth > 0;
        let trimmed = line.trim();

        if in_block_comment || starts_block_comment(trimmed) {
            if !first && !out.ends_with('\n') {
                out.push('\n');
            }
            first = false;
            out.push_str(line.trim_end());
            out.push('\n');
            block_comment_depth = update_block_comment_depth(block_comment_depth, line);
            continue;
        }

        if trimmed.is_empty() {
            if next_nonempty_is_closer(&lines, idx + 1) {
                continue;
            }
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
        block_comment_depth = update_block_comment_depth(block_comment_depth, line);
    }

    // Trim trailing newlines to avoid extra blank lines at EOF.
    while out.ends_with('\n') {
        out.pop();
    }

    out
}

fn starts_block_comment(trimmed: &str) -> bool {
    trimmed.starts_with("/*")
}

fn update_block_comment_depth(current: usize, line: &str) -> usize {
    let mut depth = current;
    let mut chars = line.chars().peekable();
    let mut in_string = false;

    while let Some(ch) = chars.next() {
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

        if ch == '/' && chars.peek() == Some(&'*') {
            chars.next();
            depth += 1;
            continue;
        }
        if ch == '*' && chars.peek() == Some(&'/') {
            chars.next();
            depth = depth.saturating_sub(1);
            continue;
        }
    }

    depth
}

fn next_nonempty_is_closer(lines: &[&str], start: usize) -> bool {
    lines[start..]
        .iter()
        .map(|line| line.trim())
        .find(|trimmed| !trimmed.is_empty())
        .is_some_and(|trimmed| {
            trimmed
                .chars()
                .next()
                .is_some_and(|ch| ch == '}' || ch == ')' || ch == ']')
        })
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

#[cfg(test)]
mod tests {
    use super::format_source;

    #[test]
    fn removes_blank_line_before_function_close() {
        let source = "fn main() with IO {\n    print(\"x\")\n\n}\n";
        let formatted = format_source(source);
        assert_eq!(formatted, "fn main() with IO {\n    print(\"x\")\n}");
    }

    #[test]
    fn preserves_indentation_inside_block_comments() {
        let source = "fn main() {\n/**\n * hello\n * world\n */\nprint(1)\n}\n";
        let formatted = format_source(source);
        assert_eq!(
            formatted,
            "fn main() {\n/**\n * hello\n * world\n */\n    print(1)\n}"
        );
    }
}
