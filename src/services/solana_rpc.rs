use serde_json::{json, Value};
use solana_sdk::{
    hash::Hash,
    pubkey::Pubkey,
    transaction::{Transaction, VersionedTransaction},
};
use std::str::FromStr;

pub struct SolanaRpc {
    url: String,
    client: reqwest::Client,
}

impl SolanaRpc {
    pub fn new(url: &str) -> Self {
        Self {
            url: url.to_string(),
            client: reqwest::Client::new(),
        }
    }

    async fn call(&self, method: &str, params: Value) -> Result<Value, String> {
        let body = json!({
            "jsonrpc": "2.0",
            "id": 1,
            "method": method,
            "params": params,
        });
        let resp = self
            .client
            .post(&self.url)
            .json(&body)
            .send()
            .await
            .map_err(|e| e.to_string())?;
        let result: Value = resp.json().await.map_err(|e| e.to_string())?;
        if let Some(err) = result.get("error") {
            return Err(format!("RPC error: {}", err));
        }
        Ok(result["result"].clone())
    }

    pub async fn account_exists(&self, address: &Pubkey) -> bool {
        let result = self
            .call(
                "getAccountInfo",
                json!([
                    address.to_string(),
                    { "encoding": "base64", "commitment": "confirmed" },
                ]),
            )
            .await;
        match result {
            Ok(val) => !val["value"].is_null(),
            Err(_) => false,
        }
    }

    pub async fn get_token_account_balance(&self, owner: &Pubkey, mint: &Pubkey) -> f64 {
        let ata = derive_associated_token_address(owner, mint);
        let result = self
            .call("getTokenAccountBalance", json!([ata.to_string()]))
            .await;
        match result {
            Ok(val) => val["value"]["uiAmount"].as_f64().unwrap_or(0.0),
            Err(_) => 0.0,
        }
    }

    pub async fn get_native_sol_balance(&self, owner: &Pubkey) -> f64 {
        let result = self
            .call(
                "getBalance",
                json!([
                    owner.to_string(),
                    { "commitment": "confirmed" },
                ]),
            )
            .await;
        match result {
            Ok(val) => {
                let lamports = val["value"]
                    .as_u64()
                    .or_else(|| val["value"].as_i64().map(|x| x.max(0) as u64))
                    .unwrap_or(0);
                lamports as f64 / 1_000_000_000.0
            }
            Err(_) => 0.0,
        }
    }

    pub async fn get_latest_blockhash(&self) -> Result<Hash, String> {
        let result = self
            .call("getLatestBlockhash", json!([{"commitment": "confirmed"}]))
            .await?;
        let hash_str = result["value"]["blockhash"]
            .as_str()
            .ok_or("No blockhash")?;
        Hash::from_str(hash_str).map_err(|e| e.to_string())
    }

    pub async fn send_and_confirm_transaction(&self, tx: &Transaction) -> Result<String, String> {
        let serialized = bincode::serialize(tx).map_err(|e| e.to_string())?;
        self.send_raw_transaction_b64(&serialized).await
    }

    pub async fn send_and_confirm_versioned_transaction(
        &self,
        tx: &VersionedTransaction,
    ) -> Result<String, String> {
        let serialized = bincode::serialize(tx).map_err(|e| e.to_string())?;
        self.send_raw_transaction_b64(&serialized).await
    }

    async fn send_raw_transaction_b64(&self, serialized: &[u8]) -> Result<String, String> {
        let encoded =
            base64::Engine::encode(&base64::engine::general_purpose::STANDARD, serialized);
        let result = self
            .call(
                "sendTransaction",
                json!([encoded, {"encoding": "base64", "preflightCommitment": "confirmed"}]),
            )
            .await?;
        result
            .as_str()
            .map(|s| s.to_string())
            .ok_or_else(|| "No signature returned".into())
    }
}

pub fn derive_associated_token_address(owner: &Pubkey, mint: &Pubkey) -> Pubkey {
    let seeds = &[owner.as_ref(), &spl_token_id().to_bytes(), mint.as_ref()];
    let program_id = associated_token_program_id();
    Pubkey::find_program_address(seeds, &program_id).0
}

fn spl_token_id() -> Pubkey {
    Pubkey::from_str("TokenkegQfeZyiNwAJbNbGKPFXCWuBvf9Ss623VQ5DA").unwrap()
}

fn associated_token_program_id() -> Pubkey {
    Pubkey::from_str("ATokenGPvbdGVxr1b2hvZbsiqW5xWH25efTNsLJA8knL").unwrap()
}
