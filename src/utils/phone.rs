use regex::Regex;

pub fn normalize_nigerian_phone(phone: &str) -> String {
    let cleaned: String = phone
        .chars()
        .filter(|c| !c.is_whitespace() && *c != '-' && *c != '(' && *c != ')')
        .collect();
    let cleaned = cleaned.strip_prefix('+').unwrap_or(&cleaned);

    if cleaned.starts_with("234") && cleaned.len() == 13 {
        return format!("+{cleaned}");
    }
    if cleaned.starts_with('0') && cleaned.len() == 11 {
        return format!("+234{}", &cleaned[1..]);
    }
    if cleaned.len() == 10 && !cleaned.starts_with('0') {
        return format!("+234{cleaned}");
    }
    format!("+{cleaned}")
}

pub fn is_valid_nigerian_phone(phone: &str) -> bool {
    let normalized = normalize_nigerian_phone(phone);
    let re = Regex::new(r"^\+234[789]\d{9}$").unwrap();
    re.is_match(&normalized)
}

pub fn phone_digits_no_plus(phone: &str) -> String {
    normalize_nigerian_phone(phone)
        .trim_start_matches('+')
        .to_string()
}

pub fn phone_local_nigeria_11_digits(phone: &str) -> String {
    let d = phone_digits_no_plus(phone);
    if d.starts_with("234") && d.len() == 13 {
        format!("0{}", &d[3..])
    } else if d.starts_with('0') && d.len() == 11 {
        d
    } else {
        d
    }
}

pub fn mask_phone(phone: &str) -> String {
    let normalized = normalize_nigerian_phone(phone);
    if normalized.len() >= 10 {
        format!(
            "{}****{}",
            &normalized[..7],
            &normalized[normalized.len() - 3..]
        )
    } else {
        normalized
    }
}
