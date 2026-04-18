use deadpool_postgres::Pool;
use uuid::Uuid;

use crate::config::AppConfig;
use crate::services::paj_orders::{fetch_user_contact, PajOrderDirection, PajOrderStatus};
use crate::services::resend_email::send_resend_email;
use crate::services::sms::send_sms;
use crate::utils::phone::normalize_nigerian_phone;

pub async fn notify_paj_order_update(
    pool: &Pool,
    config: &AppConfig,
    user_id: Uuid,
    order_id: Uuid,
    direction: PajOrderDirection,
    status: &PajOrderStatus,
    extra_line: &str,
) {
    let (email_opt, phone_raw) = match fetch_user_contact(pool, user_id).await {
        Ok(x) => x,
        Err(e) => {
            log::error!("[PAJ notify] fetch user: {e}");
            return;
        }
    };
    let dir = direction.as_str();
    let st = status.as_str();
    let subject = format!("Payce PAJ {} — {}", dir, st);
    let mut text = format!("Your PAJ {dir} order {order_id} is now: {st}.\n{extra_line}\n— Payce",);
    if text.len() > 4000 {
        text.truncate(3997);
        text.push_str("...");
    }
    let sms = format!("Payce PAJ {dir} {st}. Order {order_id}. {extra_line}");
    let sms_short: String = sms.chars().take(300).collect();

    let phone = normalize_nigerian_phone(&phone_raw);
    if !phone.is_empty() {
        send_sms(config, &phone, &sms_short).await;
    }

    if let Some(em) = email_opt {
        let e = em.trim();
        if !e.is_empty() {
            send_resend_email(config, e, &subject, &text).await;
        }
    }
}

pub async fn notify_paj_offramp_deposit_tx(
    pool: &Pool,
    config: &AppConfig,
    user_id: Uuid,
    order_id: Uuid,
    tx_signature: &str,
) {
    let (email_opt, phone_raw) = match fetch_user_contact(pool, user_id).await {
        Ok(x) => x,
        Err(e) => {
            log::error!("[PAJ notify deposit] fetch user: {e}");
            return;
        }
    };
    let subject = "Payce PAJ offramp — deposit sent on-chain";
    let text = format!(
        "Your offramp order {order_id} deposit was sent to PAJ on Solana.\nTransaction signature:\n{tx_signature}\n— Payce",
    );
    let mut sms =
        format!("Payce: Offramp {order_id} — deposit confirmed on-chain. Tx: {tx_signature}",);
    if sms.chars().count() > 300 {
        sms = sms.chars().take(297).collect::<String>();
        sms.push_str("...");
    }

    let phone = normalize_nigerian_phone(&phone_raw);
    if !phone.is_empty() {
        send_sms(config, &phone, &sms).await;
    }

    if let Some(em) = email_opt {
        let e = em.trim();
        if !e.is_empty() {
            let mut body = text;
            if body.len() > 4000 {
                body.truncate(3997);
                body.push_str("...");
            }
            send_resend_email(config, e, subject, &body).await;
        }
    }
}
