use deadpool_postgres::Pool;
use uuid::Uuid;

#[derive(Debug)]
pub enum PajSessionError {
    NotFound,
    NotVerified,
    NoToken,
    Expired,
    Db(String),
}

impl std::fmt::Display for PajSessionError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::NotFound => write!(f, "Account not found"),
            Self::NotVerified => write!(f, "PAJ email verification required"),
            Self::NoToken => write!(f, "No PAJ session token"),
            Self::Expired => write!(f, "PAJ session expired"),
            Self::Db(e) => write!(f, "{e}"),
        }
    }
}

pub async fn load_usable_paj_session_token(
    pool: &Pool,
    user_id: &Uuid,
) -> Result<String, PajSessionError> {
    let client = pool
        .get()
        .await
        .map_err(|e| PajSessionError::Db(e.to_string()))?;
    let row = client
        .query_opt(
            "SELECT paj_session_token, paj_session_expires_at, email_verified_at \
             FROM users WHERE id = $1",
            &[user_id],
        )
        .await
        .map_err(|e| PajSessionError::Db(e.to_string()))?;

    let Some(r) = row else {
        return Err(PajSessionError::NotFound);
    };
    let token: Option<String> = r.get(0);
    let expires_at: Option<chrono::DateTime<chrono::Utc>> = r.get(1);
    let email_verified_at: Option<chrono::DateTime<chrono::Utc>> = r.get(2);

    if email_verified_at.is_none() {
        return Err(PajSessionError::NotVerified);
    }
    let Some(tok) = token.filter(|t| !t.trim().is_empty()) else {
        return Err(PajSessionError::NoToken);
    };
    if let Some(exp) = expires_at {
        if exp <= chrono::Utc::now() + chrono::Duration::minutes(1) {
            return Err(PajSessionError::Expired);
        }
    }
    Ok(tok)
}
