use solana_sdk::pubkey::Pubkey;
use std::str::FromStr;
use uuid::Uuid;

use super::persist::get_favorites;

pub async fn handle_favorites(
    pool: &deadpool_postgres::Pool,
    user_id: &str,
    inputs: &[String],
) -> String {
    if inputs.len() == 1 {
        return "CON Favorites\n1. Add favorite\n2. View favorites\n3. Delete favorite".into();
    }
    let uid = Uuid::parse_str(user_id).unwrap();
    match inputs[1].as_str() {
        "1" => {
            if inputs.len() == 2 {
                return "CON Enter favorite name (e.g. Expenses):".into();
            }
            if inputs.len() == 3 {
                if inputs[2].trim().len() < 2 {
                    return "END Name too short.".into();
                }
                return "CON Enter wallet address:".into();
            }
            if inputs.len() == 4 {
                let alias = inputs[2].trim();
                let address = inputs[3].trim();
                if Pubkey::from_str(address).is_err() {
                    return "END Invalid Solana wallet address.".into();
                }
                let client = match pool.get().await {
                    Ok(c) => c,
                    Err(_) => return "END Database error. Please try again.".into(),
                };
                let exists = client
                    .query_opt(
                        "SELECT id::text FROM favorite_contacts WHERE user_id = $1 AND alias = $2",
                        &[&uid, &alias],
                    )
                    .await
                    .unwrap_or(None);
                if exists.is_some() {
                    return "END Favorite with this name already exists.".into();
                }
                let _ = client
                    .execute(
                        "INSERT INTO favorite_contacts (user_id, alias, address) VALUES ($1, $2, $3)",
                        &[&uid, &alias, &address],
                    )
                    .await;
                return format!("END Favorite saved.\n{alias} -> {address}");
            }
        }
        "2" => {
            let favs = get_favorites(pool, user_id).await;
            if favs.is_empty() {
                return "END No favorites saved yet.".into();
            }
            let lines: String = favs
                .iter()
                .enumerate()
                .take(10)
                .map(|(i, (alias, addr))| {
                    format!(
                        "{}. {alias}: {}...{}",
                        i + 1,
                        &addr[..6],
                        &addr[addr.len() - 4..]
                    )
                })
                .collect::<Vec<_>>()
                .join("\n");
            return format!("END Favorites:\n{lines}");
        }
        "3" => {
            if inputs.len() == 2 {
                return "CON Enter favorite name to delete:".into();
            }
            if inputs.len() == 3 {
                let alias = inputs[2].trim();
                let client = match pool.get().await {
                    Ok(c) => c,
                    Err(_) => return "END Database error. Please try again.".into(),
                };
                let result = client
                    .execute(
                        "DELETE FROM favorite_contacts WHERE user_id = $1 AND alias = $2",
                        &[&uid, &alias],
                    )
                    .await;
                match result {
                    Ok(count) if count > 0 => return format!("END Favorite \"{alias}\" deleted."),
                    _ => return "END Favorite not found.".into(),
                }
            }
        }
        _ => {}
    }
    "END Invalid option. Please try again.".into()
}
