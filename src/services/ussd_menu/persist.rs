use uuid::Uuid;

pub async fn user_encrypted_keypair(
    pool: &deadpool_postgres::Pool,
    user_id: &str,
) -> Option<String> {
    let uid = Uuid::parse_str(user_id).ok()?;
    let client = pool.get().await.ok()?;
    let row = client
        .query_opt("SELECT encrypted_keypair FROM users WHERE id = $1", &[&uid])
        .await
        .ok()??;
    row.get::<_, Option<String>>(0)
}

pub async fn user_phone_for_sms(pool: &deadpool_postgres::Pool, user_id: &str) -> Option<String> {
    let uid = Uuid::parse_str(user_id).ok()?;
    let client = pool.get().await.ok()?;
    let row = client
        .query_opt("SELECT phone_number FROM users WHERE id = $1", &[&uid])
        .await
        .ok()??;
    row.get(0)
}

pub async fn get_favorites(pool: &deadpool_postgres::Pool, user_id: &str) -> Vec<(String, String)> {
    let uid = Uuid::parse_str(user_id).unwrap();
    let client = match pool.get().await {
        Ok(c) => c,
        Err(_) => return vec![],
    };
    let rows = client
        .query(
            "SELECT alias, address FROM favorite_contacts WHERE user_id = $1 ORDER BY alias",
            &[&uid],
        )
        .await
        .unwrap_or_else(|_| vec![]);
    rows.into_iter().map(|r| (r.get(0), r.get(1))).collect()
}
