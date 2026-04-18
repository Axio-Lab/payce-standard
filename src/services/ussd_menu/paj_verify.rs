use deadpool_postgres::Pool;
use redis::AsyncCommands;
use redis::Client as RedisClient;
use reqwest::Client as HttpClient;
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use crate::config::AppConfig;
use crate::services::paj_ramp::{initiate_session, paj_is_configured, verify_session};
use crate::services::ussd_menu::utility_catalog::{redis_delete_key, redis_load_json};

const PENDING_TTL_SECS: u64 = 1800;

#[derive(Debug, Clone, Serialize, Deserialize)]
struct PendingRecipient {
    kind: String,
    recipient: String,
}

fn pending_key(user_id: &str) -> String {
    format!("ussd:paj_verify_pending:{user_id}")
}

async fn redis_set_pending(
    redis: &RedisClient,
    user_id: &str,
    pending: &PendingRecipient,
) -> Result<(), String> {
    let mut conn = redis
        .get_multiplexed_async_connection()
        .await
        .map_err(|e| format!("redis: {e}"))?;
    let json = serde_json::to_string(pending).map_err(|e| format!("json: {e}"))?;
    conn.set_ex::<_, _, ()>(pending_key(user_id), &json, PENDING_TTL_SECS)
        .await
        .map_err(|e| format!("redis set: {e}"))?;
    Ok(())
}

async fn redis_get_pending(redis: &RedisClient, user_id: &str) -> Option<PendingRecipient> {
    redis_load_json(redis, &pending_key(user_id)).await
}

fn device_uuid(user_id: &str) -> String {
    Uuid::parse_str(user_id)
        .map(|u| u.to_string())
        .unwrap_or_else(|_| user_id.to_string())
}

fn looks_like_email(s: &str) -> bool {
    let t = s.trim();
    if t.len() < 5 || t.len() > 320 {
        return false;
    }
    let Some((a, rest)) = t.split_once('@') else {
        return false;
    };
    !a.is_empty() && rest.contains('.') && !a.contains(' ') && !rest.contains(' ')
}

fn is_otp4(s: &str) -> bool {
    let t = s.trim();
    t.len() == 4 && t.chars().all(|c| c.is_ascii_digit())
}

fn hub_con(status_line: &str) -> String {
    format!(
        "CON PAJ (withdrawal banks)\n{status_line}\n1. Send code to email\n2. Enter OTP\n(Request 1 before 2.)"
    )
}

fn paj_session_useable(
    email_verified_at: Option<chrono::DateTime<chrono::Utc>>,
    token: Option<String>,
    expires_at: Option<chrono::DateTime<chrono::Utc>>,
) -> bool {
    if email_verified_at.is_none() {
        return false;
    }
    let Some(_tok) = token.filter(|t| !t.trim().is_empty()) else {
        return false;
    };
    if let Some(exp) = expires_at {
        if exp <= chrono::Utc::now() + chrono::Duration::minutes(1) {
            return false;
        }
    }
    true
}

async fn user_has_usable_paj_session(pool: &Pool, uid: Uuid) -> Result<bool, String> {
    let client = pool.get().await.map_err(|e| format!("pool: {e}"))?;
    let row = client
        .query_opt(
            "SELECT email_verified_at, paj_session_token, paj_session_expires_at \
             FROM users WHERE id = $1",
            &[&uid],
        )
        .await
        .map_err(|e| format!("db: {e}"))?;
    let Some(r) = row else {
        return Err("Account not found.".into());
    };
    let verified_at: Option<chrono::DateTime<chrono::Utc>> = r.get(0);
    let token: Option<String> = r.get(1);
    let expires_at: Option<chrono::DateTime<chrono::Utc>> = r.get(2);
    Ok(paj_session_useable(verified_at, token, expires_at))
}

fn end_already_verified_active() -> String {
    "END You are already verified for PAJ and your session is active.".into()
}

async fn block_if_active_paj_session(pool: &Pool, uid: Uuid) -> Option<String> {
    match user_has_usable_paj_session(pool, uid).await {
        Ok(true) => Some(end_already_verified_active()),
        Ok(false) => None,
        Err(e) if e == "Account not found." => Some("END Account not found.".into()),
        Err(e) => {
            log::error!("[PAJ verify] session check: {e}");
            Some("END Service temporarily unavailable.".into())
        }
    }
}

fn parse_paj_expires(s: &str) -> Option<chrono::DateTime<chrono::Utc>> {
    let t = s.trim();
    if t.is_empty() {
        return None;
    }
    if let Ok(dt) = chrono::DateTime::parse_from_rfc3339(t) {
        return Some(dt.with_timezone(&chrono::Utc));
    }
    t.parse::<chrono::DateTime<chrono::Utc>>().ok()
}

pub async fn handle_paj_verification_branch(
    pool: &Pool,
    redis: &RedisClient,
    http: &HttpClient,
    config: &AppConfig,
    user_id: &str,
    _phone: &str,
    inputs: &[String],
) -> String {
    if !paj_is_configured(config) {
        return "END PAJ is not configured on this server.".into();
    }

    let uid = match Uuid::parse_str(user_id) {
        Ok(u) => u,
        Err(_) => return "END Invalid account.".into(),
    };

    if inputs.len() == 2 {
        let client = match pool.get().await {
            Ok(c) => c,
            Err(e) => {
                log::error!("[PAJ verify] pool: {e}");
                return "END Service temporarily unavailable.".into();
            }
        };
        let row = client
            .query_opt(
                "SELECT email_verified_at, paj_session_token, paj_session_expires_at \
                 FROM users WHERE id = $1",
                &[&uid],
            )
            .await;
        let status_line = match row {
            Ok(Some(r)) => {
                let verified_at: Option<chrono::DateTime<chrono::Utc>> = r.get(0);
                let token: Option<String> = r.get(1);
                let expires_at: Option<chrono::DateTime<chrono::Utc>> = r.get(2);
                if paj_session_useable(verified_at, token, expires_at) {
                    "Status: verified (active session for banks)."
                } else if verified_at.is_some() {
                    "Status: verified before, but renew session (code + OTP) if expired."
                } else {
                    "Status: not verified yet."
                }
            }
            Ok(None) => return "END Account not found.".into(),
            Err(e) => {
                log::error!("[PAJ verify] query: {e}");
                return "END Database error.".into();
            }
        };
        return hub_con(status_line);
    }

    if inputs.len() < 3 {
        return hub_con("Pick an option.");
    }

    match inputs[2].as_str() {
        "1" => {
            if inputs.len() == 3 {
                if let Some(s) = block_if_active_paj_session(pool, uid).await {
                    return s;
                }
                return "CON Enter your email (for PAJ OTP):".into();
            }
            if inputs.len() > 4 {
                return "END Too many segments. Start again from My Account.".into();
            }
            let em = inputs[3].trim();
            if !looks_like_email(em) {
                return "CON Email looks invalid.\nEnter your email:".into();
            }
            if let Some(s) = block_if_active_paj_session(pool, uid).await {
                return s;
            }
            match initiate_session(http, config, Some(em), None).await {
                Ok(()) => {
                    let pr = PendingRecipient {
                        kind: "email".into(),
                        recipient: em.to_string(),
                    };
                    if let Err(e) = redis_set_pending(redis, user_id, &pr).await {
                        return format!("END {e}");
                    }
                    "END Code sent to your email.".into()
                }
                Err(e) => {
                    log::warn!("[PAJ verify] initiate email: {e}");
                    format!("END Could not send code: {e}")
                }
            }
        }
        "2" => {
            if inputs.len() == 3 {
                if let Some(s) = block_if_active_paj_session(pool, uid).await {
                    return s;
                }
                if redis_get_pending(redis, user_id).await.is_none() {
                    return "CON Request a code first.\n1. Send code to email\n2. Enter OTP\n(Pick 1, then 2.)"
                        .into();
                }
                return "CON Enter 4-digit OTP:".into();
            }
            if inputs.len() != 4 {
                return "END Invalid session. Dial 5*8 again.".into();
            }
            if let Some(s) = block_if_active_paj_session(pool, uid).await {
                return s;
            }
            let otp = inputs[3].trim();
            if !is_otp4(otp) {
                return "CON OTP must be 4 digits.\nEnter 4-digit OTP:".into();
            }
            let pending = match redis_get_pending(redis, user_id).await {
                Some(p) => p,
                None => {
                    return "END No pending code.\nDial 5*8 and pick 1 (email) first.".into();
                }
            };
            let dev = device_uuid(user_id);
            let v = if pending.kind == "email" {
                verify_session(http, config, Some(&pending.recipient), None, otp, &dev).await
            } else {
                verify_session(http, config, None, Some(&pending.recipient), otp, &dev).await
            };
            match v {
                Ok(resp) => {
                    let expires = resp
                        .expires_at
                        .as_deref()
                        .and_then(parse_paj_expires)
                        .unwrap_or_else(|| chrono::Utc::now() + chrono::Duration::days(7));
                    let client = match pool.get().await {
                        Ok(c) => c,
                        Err(e) => {
                            log::error!("[PAJ verify] save pool: {e}");
                            return "END Verified but could not save session. Try again.".into();
                        }
                    };
                    let email_patch: Option<&str> = if pending.kind == "email" {
                        Some(pending.recipient.trim())
                    } else {
                        None
                    };
                    let res = client
                        .execute(
                            "UPDATE users SET paj_session_token = $1, paj_session_expires_at = $2, \
                             email_verified_at = COALESCE(email_verified_at, NOW()), \
                             email = COALESCE(NULLIF(btrim(email), ''), NULLIF(btrim($3), '')) \
                             WHERE id = $4",
                            &[&resp.token, &expires, &email_patch, &uid],
                        )
                        .await;
                    match res {
                        Ok(_) => {
                            let _ = redis_delete_key(redis, &pending_key(user_id)).await;
                            "END Your account is verified. Withdrawal banks have been unlocked."
                                .into()
                        }
                        Err(e) => {
                            log::error!("[PAJ verify] update user: {e}");
                            "END Could not save session.".into()
                        }
                    }
                }
                Err(e) => {
                    log::warn!("[PAJ verify] verify: {e}");
                    format!("END Verification failed: {e}")
                }
            }
        }
        _ => hub_con("Invalid option."),
    }
}
