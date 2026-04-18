pub const USSD_CON_NO_BACK: &str = "CON_NOBACK\n";

pub fn with_back_option(response: &str) -> String {
    if !response.starts_with("CON ") || response.contains("\n0. Back") {
        return response.to_string();
    }
    format!("{response}\n0. Back")
}

pub fn finalize_ussd_response(response: &str) -> String {
    if let Some(body) = response.strip_prefix(USSD_CON_NO_BACK) {
        return body.to_string();
    }
    with_back_option(response)
}

pub fn parse_ussd_segments(text: &str) -> Vec<String> {
    if text.is_empty() {
        return vec![];
    }
    text.split('*')
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(String::from)
        .collect()
}

pub fn apply_back_navigation(parsed: Vec<String>) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    for seg in parsed {
        if seg == "0" {
            let _ = out.pop();
        } else {
            out.push(seg);
        }
    }
    out
}

pub fn repair_send_money_menu_pick(inputs: &[String]) -> Vec<String> {
    if inputs.len() < 3 || inputs.first().map(|s| s.as_str()) != Some("1") {
        return inputs.to_vec();
    }
    let valid_ch = |s: &str| matches!(s, "1" | "2" | "3");
    if inputs.get(1).is_some_and(|s| valid_ch(s)) {
        return inputs.to_vec();
    }
    if inputs.get(1).is_some_and(|s| !valid_ch(s)) && inputs.get(2).is_some_and(|s| valid_ch(s)) {
        let mut out = vec![inputs[0].clone(), inputs[2].clone()];
        out.extend_from_slice(&inputs[3..]);
        return out;
    }
    inputs.to_vec()
}
