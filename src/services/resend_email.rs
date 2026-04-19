use crate::config::AppConfig;
use serde::Serialize;

#[derive(Serialize)]
struct ResendPayload<'a> {
    from: &'a str,
    to: Vec<&'a str>,
    subject: &'a str,
    text: &'a str,
}

pub async fn send_resend_email(config: &AppConfig, to_email: &str, subject: &str, text: &str) {
    let key = config.resend_api_key.trim();
    if key.is_empty() {
        log::debug!("[Resend] skipped (RESEND_API_KEY empty)");
        return;
    }
    let from = config.resend_from.trim();
    if from.is_empty() {
        log::warn!("[Resend] RESEND_FROM empty");
        return;
    }
    let to = to_email.trim();
    if to.is_empty() {
        return;
    }
    let body = ResendPayload {
        from,
        to: vec![to],
        subject,
        text,
    };
    let res = config
        .http
        .post("https://api.resend.com/emails")
        .header("Authorization", format!("Bearer {key}"))
        .header("Content-Type", "application/json")
        .json(&body)
        .send()
        .await;
    match res {
        Ok(resp) => {
            let status = resp.status();
            let body_txt = resp.text().await.unwrap_or_default();
            if status.is_success() {
                log::info!("[Resend] sent to {to} status={status}");
            } else {
                log::warn!(
                    "[Resend] HTTP {} for {}: {}",
                    status,
                    to,
                    truncate_body(&body_txt, 500)
                );
            }
        }
        Err(e) => log::error!("[Resend] request failed for {to}: {e}"),
    }
}

fn truncate_body(s: &str, max: usize) -> String {
    if s.len() <= max {
        s.to_string()
    } else {
        format!("{}…", &s[..max])
    }
}
