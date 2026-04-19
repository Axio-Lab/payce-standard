use argon2::{
    password_hash::{rand_core::OsRng, PasswordHash, PasswordHasher, PasswordVerifier, SaltString},
    Argon2,
};
use redis::AsyncCommands;
use uuid::Uuid;

use crate::config::AppConfig;

pub struct PinResult {
    pub success: bool,
    pub locked: bool,
    pub attempts_remaining: Option<u32>,
}

pub async fn hash_pin(pin: &str) -> Result<String, String> {
    let pin = pin.to_string();
    tokio::task::spawn_blocking(move || {
        let salt = SaltString::generate(&mut OsRng);
        let argon2 = Argon2::default();
        argon2
            .hash_password(pin.as_bytes(), &salt)
            .map(|h| h.to_string())
            .map_err(|e| e.to_string())
    })
    .await
    .map_err(|e| e.to_string())?
}

pub async fn set_user_pin(
    pool: &deadpool_postgres::Pool,
    user_id: &str,
    pin: &str,
) -> Result<(), String> {
    let pin_hash = hash_pin(pin).await?;
    let user_uuid = Uuid::parse_str(user_id).map_err(|e| format!("Invalid user id: {e}"))?;
    let client = pool.get().await.map_err(|e| e.to_string())?;
    let updated = client
        .execute(
            "UPDATE users SET pin_hash = $1, status = 'ACTIVE' WHERE id = $2",
            &[&pin_hash, &user_uuid],
        )
        .await
        .map_err(|e| e.to_string())?;
    if updated == 0 {
        return Err("Could not set PIN: user not found".into());
    }
    Ok(())
}

pub async fn verify_pin(
    pool: &deadpool_postgres::Pool,
    redis: &redis::Client,
    config: &AppConfig,
    user_id: &str,
    pin: &str,
) -> Result<PinResult, String> {
    let user_uuid = Uuid::parse_str(user_id).map_err(|e| format!("Invalid user id: {e}"))?;
    let lockout_key = format!("pin:lockout:{user_id}");
    let attempts_key = format!("pin:attempts:{user_id}");

    let mut conn = redis
        .get_multiplexed_async_connection()
        .await
        .map_err(|e| e.to_string())?;

    let is_locked: Option<String> = conn.get(&lockout_key).await.map_err(|e| {
        log::error!("[PIN] Redis get lockout: {e}");
        format!("Redis error: {e}")
    })?;
    if is_locked.is_some() {
        return Ok(PinResult {
            success: false,
            locked: true,
            attempts_remaining: None,
        });
    }

    let client = pool.get().await.map_err(|e| e.to_string())?;
    let row = client
        .query_opt("SELECT pin_hash FROM users WHERE id = $1", &[&user_uuid])
        .await
        .map_err(|e| e.to_string())?;

    let pin_hash = match row {
        Some(row) => {
            let h: Option<String> = row.get(0);
            match h {
                Some(h) => h,
                None => {
                    return Ok(PinResult {
                        success: false,
                        locked: false,
                        attempts_remaining: None,
                    })
                }
            }
        }
        None => {
            return Ok(PinResult {
                success: false,
                locked: false,
                attempts_remaining: None,
            })
        }
    };

    let pin_owned = pin.to_string();
    let hash_owned = pin_hash.clone();
    let matched = tokio::task::spawn_blocking(move || {
        let parsed = PasswordHash::new(&hash_owned).ok();
        parsed
            .map(|h| {
                Argon2::default()
                    .verify_password(pin_owned.as_bytes(), &h)
                    .is_ok()
            })
            .unwrap_or(false)
    })
    .await
    .unwrap_or(false);

    if matched {
        let _: u64 = conn.del(&attempts_key).await.map_err(|e| {
            log::error!("[PIN] Redis del attempts: {e}");
            e.to_string()
        })?;
        return Ok(PinResult {
            success: true,
            locked: false,
            attempts_remaining: None,
        });
    }

    let attempts: u32 = conn.incr(&attempts_key, 1u32).await.map_err(|e| {
        log::error!("[PIN] Redis incr attempts: {e}");
        e.to_string()
    })?;
    conn.expire::<_, ()>(&attempts_key, (config.lockout_minutes * 60) as i64)
        .await
        .map_err(|e| {
            log::error!("[PIN] Redis expire attempts: {e}");
            e.to_string()
        })?;

    if attempts >= config.max_pin_attempts {
        conn.set_ex::<_, _, ()>(&lockout_key, "1", config.lockout_minutes * 60)
            .await
            .map_err(|e| {
                log::error!("[PIN] Redis set lockout: {e}");
                e.to_string()
            })?;
        let _: u64 = conn.del(&attempts_key).await.map_err(|e| {
            log::error!("[PIN] Redis del attempts after lock: {e}");
            e.to_string()
        })?;

        let locked_until =
            chrono::Utc::now() + chrono::Duration::minutes(config.lockout_minutes as i64);
        let _ = client
            .execute(
                "UPDATE users SET status = 'LOCKED', locked_until = $1 WHERE id = $2",
                &[&locked_until, &user_uuid],
            )
            .await;

        return Ok(PinResult {
            success: false,
            locked: true,
            attempts_remaining: None,
        });
    }

    Ok(PinResult {
        success: false,
        locked: false,
        attempts_remaining: Some(config.max_pin_attempts - attempts),
    })
}

pub fn is_valid_pin(pin: &str) -> bool {
    pin.len() == 4 && pin.chars().all(|c| c.is_ascii_digit())
}
