use crate::config::AppConfig;
use crate::utils::phone::mask_phone;

fn truncate_body(s: &str, max: usize) -> String {
    if s.len() <= max {
        s.to_string()
    } else {
        format!("{}…(truncated)", &s[..max])
    }
}

fn log_at_recipient_statuses(body: &str) {
    let v: serde_json::Value = match serde_json::from_str(body) {
        Ok(v) => v,
        Err(_) => {
            log::warn!("[SMS] Non-JSON AT response: {}", truncate_body(body, 400));
            return;
        }
    };
    let recipients = v
        .get("SMSMessageData")
        .and_then(|m| m.get("Recipients"))
        .and_then(|r| r.as_array());
    let Some(list) = recipients else {
        log::warn!(
            "[SMS] AT response (no SMSMessageData.Recipients): {}",
            truncate_body(body, 700)
        );
        return;
    };
    if list.is_empty() {
        log::warn!(
            "[SMS] AT empty Recipients list: {}",
            truncate_body(body, 700)
        );
    }
    for r in list {
        let number = r.get("number").and_then(|x| x.as_str()).unwrap_or("?");
        let status = r.get("status").and_then(|x| x.as_str()).unwrap_or("?");
        let code = r.get("statusCode").and_then(|x| x.as_i64()).or_else(|| {
            r.get("statusCode")
                .and_then(|x| x.as_u64().map(|u| u as i64))
        });
        let masked = mask_phone(number);
        if status.eq_ignore_ascii_case("Success") || status.eq_ignore_ascii_case("Sent") {
            log::info!("[SMS] AT recipient {masked} status={status} code={code:?}");
        } else {
            log::warn!(
                "[SMS] AT recipient {masked} status={status} code={code:?} — message may not be delivered (sandbox: register this number in AT dashboard)"
            );
        }
    }
}

pub async fn send_sms(config: &AppConfig, to: &str, message: &str) {
    let client = reqwest::Client::new();
    let result = client
        .post(&config.at_messaging_url)
        .header("apiKey", &config.at_api_key)
        .header("Accept", "application/json")
        .form(&[
            ("username", config.at_username.as_str()),
            ("to", to),
            ("message", message),
            ("from", config.at_sender_id.as_str()),
        ])
        .send()
        .await;

    let to_masked = mask_phone(to);
    match result {
        Ok(resp) => {
            let status = resp.status();
            let body = resp.text().await.unwrap_or_default();
            if !status.is_success() {
                log::error!(
                    "[SMS] AT HTTP {} for {}: {}",
                    status,
                    to_masked,
                    truncate_body(&body, 800)
                );
                return;
            }
            log_at_recipient_statuses(&body);
        }
        Err(e) => log::error!("[SMS] HTTP request failed for {}: {}", to_masked, e),
    }
}

pub fn tx_explorer_url(config: &AppConfig, tx_sig: &str) -> String {
    let base = config.solana_explorer_base.trim_end_matches('/');
    let network = config.solana_network.trim().to_lowercase();
    match network.as_str() {
        "mainnet" | "mainnet-beta" => format!("{base}/tx/{tx_sig}"),
        "devnet" => format!("{base}/tx/{tx_sig}?cluster=devnet"),
        "testnet" => format!("{base}/tx/{tx_sig}?cluster=testnet"),
        other => format!("{base}/tx/{tx_sig}?cluster={other}"),
    }
}

pub fn build_transfer_sent_sms(
    config: &AppConfig,
    amount: &str,
    token_amount: &str,
    stable_code: &str,
    recipient: &str,
    tx_sig: &str,
    balance: &str,
) -> String {
    let link = tx_explorer_url(config, tx_sig);
    format!(
        "Payce: You sent {amount} ({token_amount} {stable_code}) to {recipient}. Balance: {balance}. View tx: {link}"
    )
}

pub fn build_transfer_received_sms(
    config: &AppConfig,
    amount: &str,
    token_amount: &str,
    stable_code: &str,
    sender: &str,
    tx_sig: &str,
) -> String {
    let link = tx_explorer_url(config, tx_sig);
    format!(
        "Payce: You received {amount} ({token_amount} {stable_code}) from {sender}. View tx: {link}"
    )
}

pub fn build_merchant_payment_sms(
    config: &AppConfig,
    amount: &str,
    token_amount: &str,
    stable_code: &str,
    merchant_name: &str,
    tx_sig: &str,
    balance: &str,
) -> String {
    let link = tx_explorer_url(config, tx_sig);
    format!(
        "Payce: You paid {amount} ({token_amount} {stable_code}) to {merchant_name}. Balance: {balance}. View tx: {link}"
    )
}

pub fn build_merchant_receipt_sms(
    config: &AppConfig,
    amount: &str,
    token_amount: &str,
    stable_code: &str,
    customer: &str,
    tx_sig: &str,
) -> String {
    let link = tx_explorer_url(config, tx_sig);
    format!(
        "Payce: You received {amount} ({token_amount} {stable_code}) from {customer}. View tx: {link}"
    )
}
