pub fn module_binding_name(name: &str) -> &str {
    name
}

pub fn import_binding_name<'a>(name: &'a str, alias: Option<&'a str>) -> &'a str {
    alias.unwrap_or(name)
}

pub fn is_valid_module_name(name: &str) -> bool {
    let segments: Vec<&str> = name.split('.').collect();
    if segments.is_empty() {
        return false;
    }
    segments
        .iter()
        .all(|segment| is_valid_module_segment(segment))
}

pub fn is_valid_module_alias(name: &str) -> bool {
    is_valid_module_segment(name)
}

fn is_valid_module_segment(segment: &str) -> bool {
    let mut chars = segment.chars();
    let Some(first) = chars.next() else {
        return false;
    };
    if !first.is_ascii_uppercase() {
        return false;
    }
    chars.all(|ch| ch.is_ascii_alphanumeric())
}
