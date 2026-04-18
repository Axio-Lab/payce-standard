use std::collections::HashSet;

use crate::config::AppConfig;
use crate::services::airbills::{
    self, internet_list_batch_from_body, list_bet, list_cable, list_elect, list_internet,
    parse_cable_tv_list, parse_elect_disco_directory, parse_elect_plans_for_disco,
    parse_internet_plan_list_for_network, parse_vendor_product_list, InternetPlanItem,
};
use crate::services::exchange_rate::format_ngn;
use crate::services::sms::send_sms;
use crate::services::solana_rpc::SolanaRpc;
use crate::services::utility_bill::{
    network_name_to_id, purchase_airtime, purchase_betting, purchase_cable_tv, purchase_data,
    purchase_electricity,
};
use crate::utils::phone::{
    is_valid_nigerian_phone, normalize_nigerian_phone, phone_local_nigeria_11_digits,
};

use super::data_plans::{
    bucket_public_label, data_catalog_redis_key, format_data_bucket_menu_ussd,
    format_data_plan_menu_ussd, nonempty_bucket_menu, parse_data_plan_menu_input, plans_for_bucket,
    redis_load_data_catalog, redis_store_data_catalog, DataPlanCatalog, DataPlanNav,
    DATA_PLAN_PAGE_SIZE,
};
use super::persist::{user_encrypted_keypair, user_phone_for_sms};
use super::pin_gate::{log_ussd_unexpected_shape, verify_pin_or_fail};
use super::text::{
    format_bookmaker_slug_for_display, format_utility_provider_label, truncate_ussd_label,
};
use super::utility_catalog::{
    bet_catalog_key, cable_active_catalog_key, elect_active_catalog_key, elect_disco_pick_key,
    redis_delete_key, redis_load_json, redis_store_json, BetCatalog, CableCatalog, ElectCatalog,
    ElectDiscoPick,
};

pub async fn handle_pay_utility(
    pool: &deadpool_postgres::Pool,
    redis: &redis::Client,
    rpc: &SolanaRpc,
    config: &AppConfig,
    user_id: &str,
    normalized_phone: &str,
    inputs: &[String],
) -> String {
    if inputs.len() == 1 {
        return "CON Pay utility\n1. Airtime\n2. Data\n3. Electricity\n4. Betting\n5. Cable TV"
            .into();
    }
    let sub = &inputs[2..];
    match inputs[1].as_str() {
        "1" => {
            handle_utility_airtime(pool, redis, rpc, config, user_id, normalized_phone, sub).await
        }
        "2" => handle_utility_data(pool, redis, rpc, config, user_id, normalized_phone, sub).await,
        "3" => handle_utility_electricity(pool, redis, rpc, config, user_id, sub).await,
        "4" => {
            handle_utility_betting(pool, redis, rpc, config, user_id, normalized_phone, sub).await
        }
        "5" => handle_utility_cable(pool, redis, rpc, config, user_id, normalized_phone, sub).await,
        _ => "CON Invalid option.\n1. Airtime\n2. Data\n3. Electricity\n4. Betting\n5. Cable TV"
            .into(),
    }
}

async fn handle_utility_airtime(
    pool: &deadpool_postgres::Pool,
    redis: &redis::Client,
    rpc: &SolanaRpc,
    config: &AppConfig,
    user_id: &str,
    normalized_phone: &str,
    sub: &[String],
) -> String {
    if sub.is_empty() {
        return "CON Airtime: Enter 11-digit phone (e.g. 08012345678):".into();
    }
    if sub.len() == 1 {
        if !is_valid_nigerian_phone(&sub[0]) {
            return "END Invalid phone number.".into();
        }
        return format!(
            "CON Enter amount ({} - {}):",
            format_ngn(50.0),
            format_ngn(50_000.0)
        );
    }
    if sub.len() == 2 {
        let amount: f64 = match sub[1].parse() {
            Ok(a) if (50.0..=50_000.0).contains(&a) => a,
            _ => return "END Invalid amount.".into(),
        };
        let phone = normalize_nigerian_phone(&sub[0]);
        return format!(
            "CON Buy {} airtime for {}?\nEnter your PIN:",
            format_ngn(amount),
            crate::utils::phone::mask_phone(&phone)
        );
    }
    if sub.len() == 3 {
        if let Some(err) = verify_pin_or_fail(pool, redis, config, user_id, &sub[2]).await {
            return err;
        }
        let amount: f64 = sub[1].parse().unwrap_or(0.0);
        let phone = normalize_nigerian_phone(&sub[0]);
        let enc = match user_encrypted_keypair(pool, user_id).await {
            Some(e) => e,
            None => return "END Wallet not set up.".into(),
        };
        match purchase_airtime(pool, rpc, config, user_id, &enc, &phone, amount).await {
            Ok(ok) => {
                let short = &ok.chain_signature[..ok.chain_signature.len().min(8)];
                let cfg = config.clone();
                let ph = normalized_phone.to_string();
                let msg = format!(
                    "Payce utility (airtime) paid. Ref {}. On-chain {}.",
                    short,
                    &ok.chain_signature[..12.min(ok.chain_signature.len())]
                );
                tokio::spawn(async move {
                    send_sms(&cfg, &ph, &msg).await;
                });
                return format!(
                    "END Airtime purchased.\nRef: {short}\nOrder: {}\nSMS sent.",
                    &ok.airbills_id[..ok.airbills_id.len().min(8)]
                );
            }
            Err(e) => return format!("END {}", e.user_message),
        }
    }
    log_ussd_unexpected_shape("utility_airtime", sub);
    "END Something went wrong.".into()
}

async fn handle_utility_data_plan_phase(
    pool: &deadpool_postgres::Pool,
    redis: &redis::Client,
    rpc: &SolanaRpc,
    config: &AppConfig,
    user_id: &str,
    normalized_phone: &str,
    phone: &str,
    batch: &str,
    filtered: &[InternetPlanItem],
    menu_title: &str,
    rest: &[String],
) -> String {
    let nav = match parse_data_plan_menu_input(rest, filtered.len(), DATA_PLAN_PAGE_SIZE) {
        Ok(n) => n,
        Err(msg) => return format!("END {msg}"),
    };
    match nav {
        DataPlanNav::ShowMenu { page } => {
            format_data_plan_menu_ussd(filtered, page, DATA_PLAN_PAGE_SIZE, menu_title)
        }
        DataPlanNav::Picked {
            plan_index,
            consumed,
        } => {
            let plan = match filtered.get(plan_index) {
                Some(p) => p,
                None => return "END Invalid plan selection.".into(),
            };
            let prod_id = plan.prod_id.as_str();
            let tail = &rest[consumed..];

            if let Some(fixed_amount) = plan.amount_ngn.filter(|a| *a > 0.0) {
                if tail.is_empty() {
                    return format!(
                        "CON Buy data {} for {} (~{})?\nEnter your PIN:",
                        truncate_ussd_label(&plan.label, 24),
                        crate::utils::phone::mask_phone(phone),
                        format_ngn(fixed_amount)
                    );
                }
                if tail.len() == 1 {
                    if let Some(err) =
                        verify_pin_or_fail(pool, redis, config, user_id, &tail[0]).await
                    {
                        return err;
                    }
                    let enc = match user_encrypted_keypair(pool, user_id).await {
                        Some(e) => e,
                        None => return "END Wallet not set up.".into(),
                    };
                    let network = match airbills::network_checker(config, phone).await {
                        Ok(n) => network_name_to_id(&n).to_string(),
                        Err(e) => return format!("END {}", e.message),
                    };
                    return match purchase_data(
                        pool,
                        rpc,
                        config,
                        user_id,
                        &enc,
                        phone,
                        batch,
                        prod_id,
                        &network,
                        fixed_amount,
                    )
                    .await
                    {
                        Ok(ok) => {
                            let short = &ok.chain_signature[..ok.chain_signature.len().min(8)];
                            let cfg = config.clone();
                            let ph = normalized_phone.to_string();
                            let msg = format!("Payce utility (data) paid. Ref {short}.");
                            tokio::spawn(async move {
                                send_sms(&cfg, &ph, &msg).await;
                            });
                            format!(
                                "END Data purchase submitted.\nRef: {short}\nOrder: {}\nSMS sent.",
                                &ok.airbills_id[..ok.airbills_id.len().min(8)]
                            )
                        }
                        Err(e) => format!("END {}", e.user_message),
                    };
                }
                return "END Too many entries after plan choice. Go back and pick again.".into();
            }

            if tail.is_empty() {
                return "CON Plan has no fixed price. Enter amount:".into();
            }
            if tail.len() == 1 {
                let amount: f64 = match tail[0].parse() {
                    Ok(a) if a > 0.0 => a,
                    _ => return "END Invalid amount.".into(),
                };
                return format!(
                    "CON Buy data {} for {} (~{})?\nEnter your PIN:",
                    truncate_ussd_label(&plan.label, 24),
                    crate::utils::phone::mask_phone(phone),
                    format_ngn(amount)
                );
            }
            if tail.len() == 2 {
                let amount: f64 = match tail[0].parse() {
                    Ok(a) if a > 0.0 => a,
                    _ => return "END Invalid amount.".into(),
                };
                if let Some(err) = verify_pin_or_fail(pool, redis, config, user_id, &tail[1]).await
                {
                    return err;
                }
                let enc = match user_encrypted_keypair(pool, user_id).await {
                    Some(e) => e,
                    None => return "END Wallet not set up.".into(),
                };
                let network = match airbills::network_checker(config, phone).await {
                    Ok(n) => network_name_to_id(&n).to_string(),
                    Err(e) => return format!("END {}", e.message),
                };
                return match purchase_data(
                    pool, rpc, config, user_id, &enc, phone, batch, prod_id, &network, amount,
                )
                .await
                {
                    Ok(ok) => {
                        let short = &ok.chain_signature[..ok.chain_signature.len().min(8)];
                        let cfg = config.clone();
                        let ph = normalized_phone.to_string();
                        let msg = format!("Payce utility (data) paid. Ref {short}.");
                        tokio::spawn(async move {
                            send_sms(&cfg, &ph, &msg).await;
                        });
                        format!(
                            "END Data purchase submitted.\nRef: {short}\nOrder: {}\nSMS sent.",
                            &ok.airbills_id[..ok.airbills_id.len().min(8)]
                        )
                    }
                    Err(e) => format!("END {}", e.user_message),
                };
            }
            return "END Too many entries after plan choice. Go back and pick again.".into();
        }
    }
}

async fn handle_utility_data(
    pool: &deadpool_postgres::Pool,
    redis: &redis::Client,
    rpc: &SolanaRpc,
    config: &AppConfig,
    user_id: &str,
    normalized_phone: &str,
    sub: &[String],
) -> String {
    if sub.is_empty() {
        return "CON Data: Enter 11-digit phone for recipient:".into();
    }
    if sub.len() == 1 {
        if !is_valid_nigerian_phone(&sub[0]) {
            return "END Invalid phone.".into();
        }
        let phone = normalize_nigerian_phone(&sub[0]);
        let phone_local = phone_local_nigeria_11_digits(&phone);
        let network_name = match airbills::network_checker(config, &phone).await {
            Ok(n) => n,
            Err(e) => return format!("END {}", e.message),
        };
        let list_json = match list_internet(config).await {
            Ok(j) => j,
            Err(e) => {
                log::warn!("list/internet failed: {}", e.message);
                return format!("END Could not load data plans: {}", e.message);
            }
        };
        let plans = parse_internet_plan_list_for_network(&list_json, network_name.as_str());
        if plans.is_empty() {
            log::warn!("list/internet: no plans for network {}", network_name);
            return format!("END No data plans for {network_name}. Try again later.");
        }
        let batch = internet_list_batch_from_body(&list_json);
        let buckets = nonempty_bucket_menu(&plans);
        if buckets.is_empty() {
            return format!("END No data plans for {network_name}. Try again later.");
        }
        let auto_bucket = (buckets.len() == 1).then_some(buckets[0]);
        let catalog = DataPlanCatalog {
            batch,
            network_display: network_name.clone(),
            plans,
            active_bucket: auto_bucket,
        };
        let key = data_catalog_redis_key(user_id, &phone_local);
        if let Err(e) = redis_store_data_catalog(redis, &key, &catalog).await {
            log::warn!("data catalog redis: {e}");
            return "END Could not save plan list. Please try again.".into();
        }
        if let Some(b) = catalog.active_bucket {
            let filtered = plans_for_bucket(&catalog.plans, b);
            let menu_title = format!("{} — {}", network_name.trim(), bucket_public_label(b));
            return handle_utility_data_plan_phase(
                pool,
                redis,
                rpc,
                config,
                user_id,
                normalized_phone,
                &phone,
                catalog.batch.as_str(),
                &filtered,
                &menu_title,
                &[],
            )
            .await;
        }
        return format_data_bucket_menu_ussd(&network_name, &buckets, &catalog.plans);
    }

    if !is_valid_nigerian_phone(&sub[0]) {
        return "END Invalid phone.".into();
    }
    let phone = normalize_nigerian_phone(&sub[0]);
    let phone_local = phone_local_nigeria_11_digits(&phone);
    let key = data_catalog_redis_key(user_id, &phone_local);
    let Some(mut catalog) = redis_load_data_catalog(redis, &key).await else {
        return "END Plan list expired. Open Pay Utility > Data again.".into();
    };
    if catalog.plans.is_empty() {
        return "END Plan list missing. Start again from Pay Utility > Data.".into();
    }

    if catalog.active_bucket.is_none() {
        let buckets = nonempty_bucket_menu(&catalog.plans);
        let pick = sub.get(1).map(|s| s.as_str()).unwrap_or_default();
        let idx: usize = match pick.parse::<usize>() {
            Ok(n) if n >= 1 && n <= buckets.len() => n,
            _ => {
                return format!(
                    "END Pick plan type 1-{} (Daily/Weekly/Monthly/Other).",
                    buckets.len()
                );
            }
        };
        let bucket = buckets[idx - 1];
        catalog.active_bucket = Some(bucket);
        if let Err(e) = redis_store_data_catalog(redis, &key, &catalog).await {
            return format!("END Could not save session: {e}");
        }
    }

    let bucket = match catalog.active_bucket {
        Some(b) => b,
        None => return "END Pick a plan type first.".into(),
    };
    let filtered = plans_for_bucket(&catalog.plans, bucket);
    if filtered.is_empty() {
        return "END No plans in this category. Start Pay Utility > Data again.".into();
    }
    let menu_title = format!(
        "{} — {}",
        catalog.network_display.trim(),
        bucket_public_label(bucket)
    );
    let rest = if sub.len() > 2 { &sub[2..] } else { &[][..] };
    handle_utility_data_plan_phase(
        pool,
        redis,
        rpc,
        config,
        user_id,
        normalized_phone,
        &phone,
        catalog.batch.as_str(),
        &filtered,
        &menu_title,
        rest,
    )
    .await
}

async fn handle_utility_electricity(
    pool: &deadpool_postgres::Pool,
    redis: &redis::Client,
    rpc: &SolanaRpc,
    config: &AppConfig,
    user_id: &str,
    sub: &[String],
) -> String {
    let pick_key = elect_disco_pick_key(user_id);
    let active_key = elect_active_catalog_key(user_id);

    if sub.is_empty() {
        let _ = redis_delete_key(redis, &pick_key).await;
        let _ = redis_delete_key(redis, &active_key).await;
        let list_json = match list_elect(config).await {
            Ok(j) => j,
            Err(e) => {
                log::warn!("list/elect failed: {}", e.message);
                return format!("END Could not load electricity DISCO list: {}", e.message);
            }
        };
        let mut discos = parse_elect_disco_directory(&list_json);
        for d in &mut discos {
            d.label = format_utility_provider_label(&d.prod_id);
        }
        if discos.is_empty() {
            log::warn!("list/elect: no DISCO buckets parsed");
            return "END No electricity DISCOs from provider.".into();
        }
        let pick = ElectDiscoPick {
            discos,
            selected_elect_id: None,
            menu_sub_len: None,
        };
        if let Err(e) = redis_store_json(redis, &pick_key, &pick).await {
            return format!("END Could not save DISCO list: {e}");
        }
        return format_data_plan_menu_ussd(
            &pick.discos,
            0,
            DATA_PLAN_PAGE_SIZE,
            "Electricity DISCO",
        );
    }

    if let Some(mut pick) = redis_load_json::<ElectDiscoPick>(redis, &pick_key).await {
        if pick.selected_elect_id.is_none() {
            let nav = match parse_data_plan_menu_input(sub, pick.discos.len(), DATA_PLAN_PAGE_SIZE)
            {
                Ok(n) => n,
                Err(msg) => return format!("END {msg}"),
            };
            match nav {
                DataPlanNav::ShowMenu { page } => {
                    return format_data_plan_menu_ussd(
                        &pick.discos,
                        page,
                        DATA_PLAN_PAGE_SIZE,
                        "Electricity DISCO",
                    );
                }
                DataPlanNav::Picked {
                    plan_index,
                    consumed,
                } => {
                    let plan = match pick.discos.get(plan_index) {
                        Some(p) => p,
                        None => return "END Invalid DISCO selection.".into(),
                    };
                    let elect_code = plan.prod_id.clone();
                    let lbl = truncate_ussd_label(&plan.label, 22);
                    pick.selected_elect_id = Some(elect_code);
                    pick.menu_sub_len = Some(consumed);
                    if let Err(e) = redis_store_json(redis, &pick_key, &pick).await {
                        return format!("END Could not save session: {e}");
                    }
                    return format!("CON {lbl} — enter 11-digit meter number:");
                }
            }
        }

        let elect = pick.selected_elect_id.as_deref().unwrap_or("");
        let elect_trim = elect.trim();
        if elect_trim.is_empty() {
            return "END Session error. Open Pay Utility > Electricity again.".into();
        }
        let menu_len = pick.menu_sub_len.unwrap_or(0);
        if sub.len() <= menu_len {
            return "END Invalid session. Pick a DISCO again.".into();
        }
        let meter = sub[menu_len].trim();
        if meter.len() < 5 {
            return "END Enter a valid meter number.".into();
        }
        let mut validated: Vec<&'static str> = Vec::new();
        let mut first_err: Option<String> = None;
        for b in ["01", "02"] {
            match airbills::validate_elect(config, b, meter, elect_trim).await {
                Ok(_) => validated.push(b),
                Err(e) => {
                    if first_err.is_none() {
                        first_err = Some(e.message);
                    }
                }
            }
        }
        if validated.is_empty() {
            return format!(
                "END Meter check failed: {}",
                first_err.unwrap_or_else(|| "Unknown error".into())
            );
        }
        let list_json = match list_elect(config).await {
            Ok(j) => j,
            Err(e) => return format!("END Could not load electricity products: {}", e.message),
        };
        let mut plans: Vec<InternetPlanItem> = Vec::new();
        for b in &validated {
            let mut p = parse_elect_plans_for_disco(&list_json, elect_trim, *b);
            if p.is_empty() {
                p = parse_vendor_product_list(&list_json)
                    .into_iter()
                    .map(|mut it| {
                        it.batch = Some((*b).to_string());
                        it
                    })
                    .collect();
            }
            plans.extend(p);
        }
        let mut seen = HashSet::new();
        plans.retain(|pl| seen.insert(pl.prod_id.clone()));
        for p in &mut plans {
            let src = if p.label.trim().is_empty() {
                p.prod_id.as_str()
            } else {
                p.label.as_str()
            };
            p.label = format_bookmaker_slug_for_display(src);
        }
        if plans.is_empty() {
            return "END No electricity products for this DISCO. Try again later.".into();
        }
        let elect_norm = elect_trim.to_uppercase();
        let plan_sub_offset = menu_len + 1;
        let catalog = ElectCatalog {
            meter_no: meter.to_string(),
            elect_id: elect_norm,
            plans,
            plan_sub_offset: Some(plan_sub_offset),
        };
        if let Err(e) = redis_store_json(redis, &active_key, &catalog).await {
            return format!("END Could not save product list: {e}");
        }
        let _ = redis_delete_key(redis, &pick_key).await;
        return format_data_plan_menu_ussd(
            &catalog.plans,
            0,
            DATA_PLAN_PAGE_SIZE,
            "Pick a package",
        );
    }

    let Some(cat) = redis_load_json::<ElectCatalog>(redis, &active_key).await else {
        return "END Session expired. Open Pay Utility > Electricity again.".into();
    };
    if cat.plans.is_empty() {
        return "END Plan list missing. Start again from Pay Utility > Electricity.".into();
    }
    let Some(offset) = cat.plan_sub_offset else {
        return "END Session expired. Open Pay Utility > Electricity again.".into();
    };
    if sub.len() < offset {
        return "END Invalid session. Start Electricity again.".into();
    }
    let rest = &sub[offset..];
    let nav = match parse_data_plan_menu_input(rest, cat.plans.len(), DATA_PLAN_PAGE_SIZE) {
        Ok(n) => n,
        Err(msg) => return format!("END {msg}"),
    };
    let meter = cat.meter_no.as_str();
    let elect_id = cat.elect_id.as_str();
    match nav {
        DataPlanNav::ShowMenu { page } => {
            return format_data_plan_menu_ussd(
                &cat.plans,
                page,
                DATA_PLAN_PAGE_SIZE,
                "Pick a package",
            );
        }
        DataPlanNav::Picked {
            plan_index,
            consumed,
        } => {
            let plan = match cat.plans.get(plan_index) {
                Some(p) => p,
                None => return "END Invalid product selection.".into(),
            };
            let prod_id = plan.prod_id.as_str();
            let batch_sel = plan
                .batch
                .as_deref()
                .filter(|s| !s.is_empty())
                .unwrap_or("01");
            let tail = &rest[consumed..];
            if tail.is_empty() {
                let hint = truncate_ussd_label(&plan.label, 28);
                return format!("CON {} — enter amount (min {}):", hint, format_ngn(2000.0));
            }
            if tail.len() == 1 {
                let amount: f64 = match tail[0].parse() {
                    Ok(a) if a >= 2000.0 => a,
                    _ => return "END Invalid amount.".into(),
                };
                return format!(
                    "CON Pay {} electricity for meter {}...\nEnter your PIN:",
                    format_ngn(amount),
                    &meter[meter.len().saturating_sub(4)..]
                );
            }
            if tail.len() == 2 {
                let amount: f64 = match tail[0].parse() {
                    Ok(a) if a >= 2000.0 => a,
                    _ => return "END Invalid amount.".into(),
                };
                if let Some(err) = verify_pin_or_fail(pool, redis, config, user_id, &tail[1]).await
                {
                    return err;
                }
                let enc = match user_encrypted_keypair(pool, user_id).await {
                    Some(e) => e,
                    None => return "END Wallet not set up.".into(),
                };
                match purchase_electricity(
                    pool, rpc, config, user_id, &enc, meter, elect_id, batch_sel, prod_id, amount,
                )
                .await
                {
                    Ok(ok) => {
                        let short = &ok.chain_signature[..ok.chain_signature.len().min(8)];
                        let cfg = config.clone();
                        let ph = user_phone_for_sms(pool, user_id).await.unwrap_or_default();
                        let msg = format!("Payce utility (electricity) paid. Ref {short}.");
                        tokio::spawn(async move {
                            if !ph.is_empty() {
                                send_sms(&cfg, &ph, &msg).await;
                            }
                        });
                        let _ = redis_delete_key(redis, &active_key).await;
                        format!(
                            "END Electricity token request sent.\nRef: {short}\nOrder: {}\nSMS sent.",
                            &ok.airbills_id[..ok.airbills_id.len().min(8)]
                        )
                    }
                    Err(e) => format!("END {}", e.user_message),
                }
            } else {
                "END Too many entries after product choice. Start again.".into()
            }
        }
    }
}

async fn handle_utility_betting(
    pool: &deadpool_postgres::Pool,
    redis: &redis::Client,
    rpc: &SolanaRpc,
    config: &AppConfig,
    user_id: &str,
    normalized_phone: &str,
    sub: &[String],
) -> String {
    let key = bet_catalog_key(user_id);

    if sub.is_empty() {
        let list_json = match list_bet(config).await {
            Ok(j) => j,
            Err(e) => {
                log::warn!("list/bet failed: {}", e.message);
                return format!("END Could not load betting products: {}", e.message);
            }
        };
        let mut plans = parse_vendor_product_list(&list_json);
        if plans.is_empty() {
            log::warn!("list/bet: no rows after parse");
            return "END No betting products from provider.".into();
        }
        for p in &mut plans {
            p.label = format_bookmaker_slug_for_display(&p.prod_id);
        }
        let catalog = BetCatalog {
            customer_id: String::new(),
            plans,
            selected_prod_id: None,
            menu_sub_len: None,
        };
        if let Err(e) = redis_store_json(redis, &key, &catalog).await {
            return format!("END Could not save product list: {e}");
        }
        return format_data_plan_menu_ussd(&catalog.plans, 0, DATA_PLAN_PAGE_SIZE, "Betting");
    }

    let Some(mut cat) = redis_load_json::<BetCatalog>(redis, &key).await else {
        return "END Product list expired. Open Pay Utility > Betting again.".into();
    };
    if cat.plans.is_empty() {
        return "END Plan list missing. Start again from Pay Utility > Betting.".into();
    }

    if cat.selected_prod_id.is_none() {
        let nav = match parse_data_plan_menu_input(sub, cat.plans.len(), DATA_PLAN_PAGE_SIZE) {
            Ok(n) => n,
            Err(msg) => return format!("END {msg}"),
        };
        match nav {
            DataPlanNav::ShowMenu { page } => {
                return format_data_plan_menu_ussd(
                    &cat.plans,
                    page,
                    DATA_PLAN_PAGE_SIZE,
                    "Betting",
                );
            }
            DataPlanNav::Picked {
                plan_index,
                consumed,
            } => {
                let plan = match cat.plans.get(plan_index) {
                    Some(p) => p,
                    None => return "END Invalid bookmaker selection.".into(),
                };
                let label = truncate_ussd_label(&plan.label, 22);
                cat.selected_prod_id = Some(plan.prod_id.clone());
                cat.menu_sub_len = Some(consumed);
                if let Err(e) = redis_store_json(redis, &key, &cat).await {
                    return format!("END Could not save session: {e}");
                }
                return format!(
                    "CON {} — enter your bookmaker customer / user ID (one segment, no *):",
                    label
                );
            }
        }
    }

    let menu_len = cat.menu_sub_len.unwrap_or(0);
    let prod_id = match cat.selected_prod_id.as_deref() {
        Some(p) if !p.is_empty() => p,
        _ => return "END Session error. Start Betting again.".into(),
    };

    if cat.customer_id.is_empty() {
        if sub.len() <= menu_len {
            return "END Invalid session. Pick a bookmaker again.".into();
        }
        let cid_raw = sub[menu_len].trim();
        if cid_raw.is_empty() || cid_raw.contains('*') {
            return "END Invalid customer ID. Use one segment without *.".into();
        }
        cat.customer_id = cid_raw.to_string();
        if let Err(e) = redis_store_json(redis, &key, &cat).await {
            return format!("END Could not save session: {e}");
        }
        return format!(
            "CON Enter amount ({} - {}):",
            format_ngn(1_000.0),
            format_ngn(100_000.0)
        );
    }

    let cid = cat.customer_id.trim();
    let tail = &sub[(menu_len + 1)..];

    if tail.is_empty() {
        return format!(
            "CON Enter amount ({} - {}):",
            format_ngn(1_000.0),
            format_ngn(100_000.0)
        );
    }
    if tail.len() == 1 {
        let amount: f64 = match tail[0].parse() {
            Ok(a) if (1_000.0..=100_000.0).contains(&a) => a,
            _ => {
                return format!(
                    "END Amount must be {} - {}.",
                    format_ngn(1_000.0),
                    format_ngn(100_000.0)
                );
            }
        };
        let hint = if cid.len() > 4 {
            format!("…{}", &cid[cid.len().saturating_sub(4)..])
        } else {
            "****".into()
        };
        let plan_label = cat
            .plans
            .iter()
            .find(|p| p.prod_id == prod_id)
            .map(|p| truncate_ussd_label(&p.label, 20))
            .unwrap_or_else(|| prod_id.into());
        return format!(
            "CON Fund betting acct {hint} ({}) for {}?\nEnter your PIN:",
            plan_label,
            format_ngn(amount)
        );
    }
    if tail.len() == 2 {
        let amount: f64 = match tail[0].parse() {
            Ok(a) if (1_000.0..=100_000.0).contains(&a) => a,
            _ => {
                return format!(
                    "END Amount must be {} - {}.",
                    format_ngn(1_000.0),
                    format_ngn(100_000.0)
                );
            }
        };
        if let Some(err) = verify_pin_or_fail(pool, redis, config, user_id, &tail[1]).await {
            return err;
        }
        let enc = match user_encrypted_keypair(pool, user_id).await {
            Some(e) => e,
            None => return "END Wallet not set up.".into(),
        };
        return match purchase_betting(pool, rpc, config, user_id, &enc, cid, prod_id, amount).await
        {
            Ok(ok) => {
                let short = &ok.chain_signature[..ok.chain_signature.len().min(8)];
                let cfg = config.clone();
                let ph = normalized_phone.to_string();
                let msg = format!("Payce utility (betting) paid. Ref {short}.");
                tokio::spawn(async move {
                    send_sms(&cfg, &ph, &msg).await;
                });
                format!(
                    "END Betting top-up submitted.\nRef: {short}\nOrder: {}\nSMS sent.",
                    &ok.airbills_id[..ok.airbills_id.len().min(8)]
                )
            }
            Err(e) => format!("END {}", e.user_message),
        };
    }
    "END Too many entries after amount. Start again.".into()
}

async fn handle_utility_cable(
    pool: &deadpool_postgres::Pool,
    redis: &redis::Client,
    rpc: &SolanaRpc,
    config: &AppConfig,
    user_id: &str,
    normalized_phone: &str,
    sub: &[String],
) -> String {
    let active_key = cable_active_catalog_key(user_id);

    if sub.is_empty() {
        let _ = redis_delete_key(redis, &active_key).await;
        let list_json = match list_cable(config).await {
            Ok(j) => j,
            Err(e) => {
                log::warn!("list/cable failed: {}", e.message);
                return format!("END Could not load cable products: {}", e.message);
            }
        };
        let mut plans = parse_cable_tv_list(&list_json);
        if plans.is_empty() {
            log::warn!("list/cable: no package rows after parse");
            return "END No cable products from provider.".into();
        }
        for p in &mut plans {
            let src = if p.label.trim().is_empty() {
                p.prod_id.as_str()
            } else {
                p.label.as_str()
            };
            p.label = format_bookmaker_slug_for_display(src);
        }
        let cat = CableCatalog {
            smart_card: String::new(),
            plans,
            selected_prod_id: None,
            menu_sub_len: None,
        };
        if let Err(e) = redis_store_json(redis, &active_key, &cat).await {
            return format!("END Could not save product list: {e}");
        }
        return format_data_plan_menu_ussd(&cat.plans, 0, DATA_PLAN_PAGE_SIZE, "Cable TV");
    }

    let Some(mut cat) = redis_load_json::<CableCatalog>(redis, &active_key).await else {
        return "END Session expired. Open Pay Utility > Cable TV again.".into();
    };
    if cat.plans.is_empty() {
        return "END Plan list missing. Start again from Pay Utility > Cable TV.".into();
    }

    if cat.selected_prod_id.is_none() {
        let nav = match parse_data_plan_menu_input(sub, cat.plans.len(), DATA_PLAN_PAGE_SIZE) {
            Ok(n) => n,
            Err(msg) => return format!("END {msg}"),
        };
        match nav {
            DataPlanNav::ShowMenu { page } => {
                return format_data_plan_menu_ussd(
                    &cat.plans,
                    page,
                    DATA_PLAN_PAGE_SIZE,
                    "Cable TV",
                );
            }
            DataPlanNav::Picked {
                plan_index,
                consumed,
            } => {
                let plan = match cat.plans.get(plan_index) {
                    Some(p) => p,
                    None => return "END Invalid product selection.".into(),
                };
                cat.selected_prod_id = Some(plan.prod_id.clone());
                cat.menu_sub_len = Some(consumed);
                if let Err(e) = redis_store_json(redis, &active_key, &cat).await {
                    return format!("END Could not save session: {e}");
                }
                return "CON Enter smart card number:".into();
            }
        }
    }

    let menu_len = cat.menu_sub_len.unwrap_or(0);
    let prod_id = match cat.selected_prod_id.as_deref() {
        Some(p) if !p.is_empty() => p,
        _ => return "END Session error. Start Cable TV again.".into(),
    };

    if cat.smart_card.trim().is_empty() {
        if sub.len() <= menu_len {
            return "END Invalid session. Pick a package again.".into();
        }
        let card_in = sub[menu_len].trim();
        if card_in.is_empty() {
            return "END Invalid smart card number.".into();
        }
        cat.smart_card = card_in.to_string();
        if let Err(e) = redis_store_json(redis, &active_key, &cat).await {
            return format!("END Could not save session: {e}");
        }
        return "CON Enter 11-digit phone (account contact):".into();
    }

    let card = cat.smart_card.trim();
    let rest = &sub[menu_len + 1..];

    if rest.is_empty() {
        return "CON Enter 11-digit phone (account contact):".into();
    }
    if rest.len() == 1 {
        if !is_valid_nigerian_phone(&rest[0]) {
            return "END Invalid phone.".into();
        }
        return format!(
            "CON Enter amount ({} - {}):",
            format_ngn(100.0),
            format_ngn(500_000.0)
        );
    }
    if rest.len() == 2 {
        let phone = normalize_nigerian_phone(&rest[0]);
        let phone_local = phone_local_nigeria_11_digits(&phone);
        if phone_local.len() != 11 {
            return "END Invalid phone.".into();
        }
        let amount: f64 = match rest[1].parse() {
            Ok(a) if (100.0..=500_000.0).contains(&a) => a,
            _ => {
                return format!(
                    "END Amount must be {} - {}.",
                    format_ngn(100.0),
                    format_ngn(500_000.0)
                );
            }
        };
        let plan_label = cat
            .plans
            .iter()
            .find(|p| p.prod_id == prod_id)
            .map(|p| truncate_ussd_label(&p.label, 18))
            .unwrap_or_else(|| prod_id.into());
        let card_hint = if card.len() > 4 {
            format!("…{}", &card[card.len().saturating_sub(4)..])
        } else {
            "****".into()
        };
        return format!(
            "CON Pay cable {card_hint} {} for {}?\nEnter your PIN:",
            plan_label,
            format_ngn(amount)
        );
    }
    if rest.len() == 3 {
        let phone = normalize_nigerian_phone(&rest[0]);
        let amount: f64 = match rest[1].parse() {
            Ok(a) if (100.0..=500_000.0).contains(&a) => a,
            _ => {
                return format!(
                    "END Amount must be {} - {}.",
                    format_ngn(100.0),
                    format_ngn(500_000.0)
                );
            }
        };
        if let Some(err) = verify_pin_or_fail(pool, redis, config, user_id, &rest[2]).await {
            return err;
        }
        let enc = match user_encrypted_keypair(pool, user_id).await {
            Some(e) => e,
            None => return "END Wallet not set up.".into(),
        };
        return match purchase_cable_tv(
            pool, rpc, config, user_id, &enc, card, &phone, prod_id, amount,
        )
        .await
        {
            Ok(ok) => {
                let short = &ok.chain_signature[..ok.chain_signature.len().min(8)];
                let cfg = config.clone();
                let ph = normalized_phone.to_string();
                let msg = format!("Payce utility (cable) paid. Ref {short}.");
                tokio::spawn(async move {
                    send_sms(&cfg, &ph, &msg).await;
                });
                let _ = redis_delete_key(redis, &active_key).await;
                format!(
                    "END Cable TV payment submitted.\nRef: {short}\nOrder: {}\nSMS sent.",
                    &ok.airbills_id[..ok.airbills_id.len().min(8)]
                )
            }
            Err(e) => format!("END {}", e.user_message),
        };
    }
    "END Too many entries after product choice. Start again.".into()
}
