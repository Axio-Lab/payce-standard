use rand::Rng;
use uuid::Uuid;

pub fn generate_merchant_code() -> String {
    let code: u32 = rand::thread_rng().gen_range(100_000..1_000_000);
    code.to_string()
}

const MAX_BUSINESS_NAME_LEN: usize = 120;

pub async fn register_merchant(
    pool: &deadpool_postgres::Pool,
    user_id: &str,
    business_name: &str,
    category: &str,
) -> Result<String, String> {
    let business_name = business_name.trim();
    if business_name.len() < 2 {
        return Err("Business name too short".into());
    }
    if business_name.len() > MAX_BUSINESS_NAME_LEN {
        return Err("Business name too long".into());
    }
    let client = pool.get().await.map_err(|e| e.to_string())?;
    let mut code = generate_merchant_code();
    loop {
        let exists = client
            .query_opt(
                "SELECT id::text FROM merchants WHERE merchant_code = $1",
                &[&code],
            )
            .await
            .map_err(|e| e.to_string())?;
        if exists.is_none() {
            break;
        }
        code = generate_merchant_code();
    }

    let uid = Uuid::parse_str(user_id).map_err(|e| format!("Invalid user id: {e}"))?;
    client
        .execute(
            "INSERT INTO merchants (user_id, merchant_code, business_name, category, status) \
             VALUES ($1, $2, $3, $4, 'ACTIVE')",
            &[&uid, &code, &business_name, &category],
        )
        .await
        .map_err(|e| e.to_string())?;

    Ok(code)
}

#[derive(Debug)]
pub struct MerchantInfo {
    pub merchant_code: String,
    pub business_name: String,
    pub category: String,
    pub status: String,
}

pub async fn get_merchant_by_code(
    pool: &deadpool_postgres::Pool,
    code: &str,
) -> Option<MerchantInfo> {
    let client = pool.get().await.ok()?;
    let row = client
        .query_opt(
            "SELECT merchant_code, business_name, category, status::text \
             FROM merchants WHERE merchant_code = $1",
            &[&code],
        )
        .await
        .ok()?;

    row.map(|r| MerchantInfo {
        merchant_code: r.get(0),
        business_name: r.get(1),
        category: r.get(2),
        status: r.get(3),
    })
}

pub async fn get_merchant_by_user_id(
    pool: &deadpool_postgres::Pool,
    user_id: &str,
) -> Option<(String, String)> {
    let uid = Uuid::parse_str(user_id).ok()?;
    let client = pool.get().await.ok()?;
    let row = client
        .query_opt(
            "SELECT business_name, merchant_code FROM merchants WHERE user_id = $1",
            &[&uid],
        )
        .await
        .ok()?;
    row.map(|r| (r.get(0), r.get(1)))
}
