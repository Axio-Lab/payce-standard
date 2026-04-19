pub fn wrap_for_ussd(value: &str, chunk_size: usize) -> String {
    value
        .as_bytes()
        .chunks(chunk_size)
        .map(|c| std::str::from_utf8(c).unwrap_or(""))
        .collect::<Vec<_>>()
        .join("\n")
}

pub fn sanitize_ussd_ascii(s: &str) -> String {
    s.chars()
        .map(|c| match c {
            '≈' => '~',
            '\n' | '\r' => ' ',
            c if c.is_ascii() && (c.is_ascii_graphic() || c == ' ') => c,
            _ => ' ',
        })
        .collect::<String>()
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
}

pub fn truncate_ussd_label(s: &str, max_chars: usize) -> String {
    let count = s.chars().count();
    if count <= max_chars {
        s.to_string()
    } else {
        format!(
            "{}…",
            s.chars()
                .take(max_chars.saturating_sub(1))
                .collect::<String>()
        )
    }
}

pub fn format_utility_provider_label(raw: &str) -> String {
    let t = raw.trim();
    if t.is_empty() {
        return String::new();
    }
    let letters_only = t.chars().all(|c| c.is_ascii_alphabetic());
    if letters_only && (3..=8).contains(&t.chars().count()) {
        return t.to_uppercase();
    }
    format_bookmaker_slug_for_display(t)
}

pub fn format_bookmaker_slug_for_display(slug: &str) -> String {
    let slug = slug.trim();
    if slug.is_empty() {
        return String::new();
    }
    slug.split(['-', '_'])
        .filter(|s| !s.is_empty())
        .map(|segment| {
            let mut out = String::new();
            let mut capitalized_first_alpha = false;
            for c in segment.chars() {
                if c.is_ascii_alphabetic() {
                    if !capitalized_first_alpha {
                        out.extend(c.to_uppercase());
                        capitalized_first_alpha = true;
                    } else {
                        out.push(c.to_ascii_lowercase());
                    }
                } else {
                    out.push(c);
                }
            }
            out
        })
        .collect::<Vec<_>>()
        .join(" ")
}
