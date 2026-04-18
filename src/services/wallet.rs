use hkdf::Hkdf;
use sha2::Sha256;
use solana_sdk::{
    pubkey::Pubkey,
    signature::{Keypair, SeedDerivable},
    signer::Signer,
    transaction::Transaction,
};
use spl_associated_token_account::instruction::create_associated_token_account;
use uuid::Uuid;

use crate::config::AppConfig;
use crate::services::solana_rpc::{derive_associated_token_address, SolanaRpc};
use crate::utils::crypto::{decrypt, encrypt};

fn derive_keypair(master_seed: &str, phone_number: &str) -> Keypair {
    let seed_bytes = hex::decode(master_seed).expect("Invalid master seed hex");
    let hk = Hkdf::<Sha256>::new(Some(b"payce-ng-v1"), &seed_bytes);
    let mut derived = [0u8; 32];
    hk.expand(phone_number.as_bytes(), &mut derived)
        .expect("HKDF expand failed");
    Keypair::from_seed(&derived).expect("Invalid seed for keypair")
}

pub async fn create_spl_ata_for_owner_if_missing(
    rpc: &SolanaRpc,
    config: &AppConfig,
    owner: &Pubkey,
    mint: &Pubkey,
) -> Result<(), String> {
    let ata = derive_associated_token_address(owner, mint);
    if rpc.account_exists(&ata).await {
        return Ok(());
    }
    let ix =
        create_associated_token_account(&config.fee_payer.pubkey(), owner, mint, &spl_token::id());
    let blockhash = rpc.get_latest_blockhash().await?;
    let tx = Transaction::new_signed_with_payer(
        &[ix],
        Some(&config.fee_payer.pubkey()),
        &[&*config.fee_payer],
        blockhash,
    );
    rpc.send_and_confirm_transaction(&tx).await?;
    Ok(())
}

pub async fn create_wallet(
    pool: &deadpool_postgres::Pool,
    config: &AppConfig,
    rpc: &SolanaRpc,
    user_id: &str,
    phone_number: &str,
) -> Result<String, String> {
    let user_uuid = Uuid::parse_str(user_id).map_err(|e| format!("Invalid user id: {e}"))?;
    let keypair = derive_keypair(&config.wallet_master_seed, phone_number);
    let owner = keypair.pubkey();
    for s in &config.stable_coins {
        create_spl_ata_for_owner_if_missing(rpc, config, &owner, &s.mint).await?;
    }
    let public_key = owner.to_string();
    let secret_b58 = bs58::encode(keypair.to_bytes()).into_string();
    let encrypted =
        encrypt(&secret_b58, &config.wallet_encryption_key).map_err(|e| e.to_string())?;

    let client = pool.get().await.map_err(|e| e.to_string())?;
    client
        .execute(
            "UPDATE users SET solana_pubkey = $1, encrypted_keypair = $2 WHERE id = $3",
            &[&public_key, &encrypted, &user_uuid],
        )
        .await
        .map_err(|e| e.to_string())?;

    Ok(public_key)
}

pub fn get_keypair_for_user(
    encrypted_keypair: &str,
    encryption_key: &str,
) -> Result<Keypair, String> {
    let secret_b58 = decrypt(encrypted_keypair, encryption_key).map_err(|e| e.to_string())?;
    let bytes = bs58::decode(&secret_b58)
        .into_vec()
        .map_err(|e| e.to_string())?;
    Keypair::from_bytes(&bytes).map_err(|e| e.to_string())
}

pub async fn get_spl_token_balance(rpc: &SolanaRpc, wallet_address: &str, mint: &Pubkey) -> f64 {
    let owner = match wallet_address.parse::<Pubkey>() {
        Ok(pk) => pk,
        Err(_) => return 0.0,
    };
    rpc.get_token_account_balance(&owner, mint).await
}

pub async fn get_native_sol_balance(rpc: &SolanaRpc, wallet_address: &str) -> f64 {
    let owner = match wallet_address.parse::<Pubkey>() {
        Ok(pk) => pk,
        Err(_) => return 0.0,
    };
    rpc.get_native_sol_balance(&owner).await
}

pub async fn export_private_key(
    pool: &deadpool_postgres::Pool,
    user_id: &str,
    encryption_key: &str,
) -> Result<Option<String>, String> {
    let user_uuid = Uuid::parse_str(user_id).map_err(|e| format!("Invalid user id: {e}"))?;
    let client = pool.get().await.map_err(|e| e.to_string())?;
    let row = client
        .query_opt(
            "SELECT encrypted_keypair FROM users WHERE id = $1",
            &[&user_uuid],
        )
        .await
        .map_err(|e| e.to_string())?;

    match row {
        Some(row) => {
            let encrypted: Option<String> = row.get(0);
            match encrypted {
                Some(enc) => {
                    let key = decrypt(&enc, encryption_key).map_err(|e| e.to_string())?;
                    Ok(Some(key))
                }
                None => Ok(None),
            }
        }
        _ => Ok(None),
    }
}

pub async fn import_private_key(
    pool: &deadpool_postgres::Pool,
    config: &AppConfig,
    rpc: &SolanaRpc,
    user_id: &str,
    private_key: &str,
) -> Result<String, String> {
    let user_uuid = Uuid::parse_str(user_id).map_err(|e| format!("Invalid user id: {e}"))?;
    let client = pool.get().await.map_err(|e| e.to_string())?;
    let current = client
        .query_opt(
            "SELECT solana_pubkey, encrypted_keypair FROM users WHERE id = $1",
            &[&user_uuid],
        )
        .await
        .map_err(|e| e.to_string())?;

    let bytes = bs58::decode(private_key.trim())
        .into_vec()
        .map_err(|e| format!("Invalid private key: {e}"))?;
    let keypair = Keypair::from_bytes(&bytes).map_err(|e| format!("Invalid keypair: {e}"))?;
    for s in &config.stable_coins {
        create_spl_ata_for_owner_if_missing(rpc, config, &keypair.pubkey(), &s.mint).await?;
    }
    let public_key = keypair.pubkey().to_string();
    let secret_b58 = bs58::encode(keypair.to_bytes()).into_string();
    let encrypted =
        encrypt(&secret_b58, &config.wallet_encryption_key).map_err(|e| e.to_string())?;

    let (old_pubkey, old_encrypted): (Option<String>, Option<String>) = match current {
        Some(row) => (row.get(0), row.get(1)),
        None => (None, None),
    };

    client
        .execute(
            "UPDATE users SET solana_pubkey = $1, encrypted_keypair = $2, \
             secondary_solana_pubkey = $3, secondary_encrypted_keypair = $4 \
             WHERE id = $5",
            &[
                &public_key,
                &encrypted,
                &old_pubkey,
                &old_encrypted,
                &user_uuid,
            ],
        )
        .await
        .map_err(|e| e.to_string())?;

    Ok(public_key)
}
