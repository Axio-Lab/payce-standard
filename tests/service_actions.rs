use payce_ng::middleware::auth::ct_eq_str;
use payce_ng::routes::ussd::redact_text;
use payce_ng::services::airbills::{
    list_response_data_len, parse_cable_tv_list, parse_elect_disco_directory,
    parse_elect_plans_for_disco, parse_internet_plan_list, parse_internet_plan_list_for_network,
    InternetPlanItem,
};
use payce_ng::services::jupiter_price::parse_usd_price_body;
use payce_ng::services::paj_rates::parse_rates_body;
use payce_ng::services::ussd_menu::{
    classify_data_plan_bucket, format_bookmaker_slug_for_display, format_utility_provider_label,
    is_nuban_10, nonempty_bucket_menu, DataPlanBucket,
};
use payce_ng::utils::validation::is_valid_merchant_code_param;
use serde_json::json;

#[test]
fn ct_eq_matches_and_mismatches() {
    assert!(ct_eq_str("abc123", "abc123"));
    assert!(!ct_eq_str("abc123", "abc124"));
    assert!(!ct_eq_str("short", "longer"));
}

#[test]
fn redact_text_masks_and_redacts() {
    assert_eq!(redact_text("1*1234*2"), "1******2");
    assert_eq!(
        redact_text("menu*abcdefghijklmnopqrstuvwxyz1234567890"),
        "menu*abcdef...REDACTED"
    );
}

#[test]
fn merchant_code_param_validation() {
    assert!(is_valid_merchant_code_param("123456"));
    assert!(is_valid_merchant_code_param(" 123456 "));
    assert!(!is_valid_merchant_code_param("12345"));
    assert!(!is_valid_merchant_code_param("1234567"));
    assert!(!is_valid_merchant_code_param("12a456"));
}

#[test]
fn bookmaker_slug_title_case() {
    assert_eq!(format_bookmaker_slug_for_display("supabet"), "Supabet");
    assert_eq!(format_bookmaker_slug_for_display("1xbet"), "1Xbet");
    assert_eq!(
        format_bookmaker_slug_for_display("bet9ja-agent"),
        "Bet9ja Agent"
    );
    assert_eq!(
        format_bookmaker_slug_for_display("western-lotto"),
        "Western Lotto"
    );
}

#[test]
fn utility_provider_label_caps_disco() {
    assert_eq!(format_utility_provider_label("EKEDC"), "EKEDC");
    assert_eq!(format_utility_provider_label("ikedc"), "IKEDC");
}

#[test]
fn nuban_accepts_ten_digits() {
    assert!(is_nuban_10("0123456789"));
    assert!(!is_nuban_10("012345678"));
    assert!(!is_nuban_10("01234567890"));
    assert!(!is_nuban_10("01234a6789"));
}

#[test]
fn parse_rates_body_ok() {
    let body = r#"{
        "onRampRate": { "rate": 1510 },
        "offRampRate": { "rate": 1525 }
    }"#;
    let out = parse_rates_body(body).expect("must parse");
    assert_eq!(out.on_ramp, 1510.0);
    assert_eq!(out.off_ramp, 1525.0);
}

#[test]
fn parse_rates_body_rejects_non_positive_and_bad_shape() {
    let non_positive = r#"{
        "onRampRate": { "rate": 0 },
        "offRampRate": { "rate": 1525 }
    }"#;
    assert!(parse_rates_body(non_positive).is_err());
    assert!(parse_rates_body(r#"{"onRampRate":{"rate":1510}}"#).is_err());
}

#[test]
fn parse_usd_price_body_cases() {
    let mint = "So11111111111111111111111111111111111111112";
    let ok_body = r#"{
        "So11111111111111111111111111111111111111112": {
            "usdPrice": 195.72
        }
    }"#;
    let out = parse_usd_price_body(ok_body, mint).expect("price should parse");
    assert_eq!(out, 195.72);
    assert!(parse_usd_price_body(r#"{"OtherMint":{"usdPrice": 1.0}}"#, mint).is_err());
    assert!(parse_usd_price_body(
        r#"{"So11111111111111111111111111111111111111112":{"usdPrice": 0}}"#,
        mint
    )
    .is_err());
}

fn plan(label: &str, prod_id: &str) -> InternetPlanItem {
    InternetPlanItem {
        prod_id: prod_id.to_string(),
        label: label.to_string(),
        amount_ngn: Some(500.0),
        batch: None,
    }
}

#[test]
fn data_plan_bucket_classification_and_menu() {
    assert_eq!(
        classify_data_plan_bucket(&plan("1GB 30 Days", "p1")),
        DataPlanBucket::Monthly
    );
    assert_eq!(
        classify_data_plan_bucket(&plan("6GB 7 Days", "w")),
        DataPlanBucket::Weekly
    );
    assert_eq!(
        classify_data_plan_bucket(&plan("350MB 1 Day", "d")),
        DataPlanBucket::Daily
    );

    let plans = vec![plan("Daily pack", "a"), plan("30 Days big", "b")];
    let m = nonempty_bucket_menu(&plans);
    assert_eq!(m.len(), 2);
    assert!(m.contains(&DataPlanBucket::Daily));
    assert!(m.contains(&DataPlanBucket::Monthly));
}

#[test]
fn airbills_parsing_cases() {
    let v = json!({
        "status": "00",
        "data": [
            {"prodId": "p1", "name": "1GB", "amount": 350},
            {"product_code": 22, "title": "2GB", "price": "500"}
        ]
    });
    let plans = parse_internet_plan_list(&v);
    assert_eq!(plans.len(), 2);
    assert_eq!(plans[0].prod_id, "p1");
    assert_eq!(plans[1].prod_id, "22");

    let v = json!({
        "status": "00",
        "data": { "plans": [{"id": "x", "description": "Combo", "cost": 1000}] }
    });
    let plans = parse_internet_plan_list(&v);
    assert_eq!(plans.len(), 1);
    assert_eq!(plans[0].prod_id, "x");

    let v = json!({
        "status": "00",
        "data": {
            "providers": [
                {"prodId": "bet9ja", "name": "Bet9ja"},
                {"productId": "sporty", "title": "SportyBet", "amount": 0}
            ]
        }
    });
    let plans = parse_internet_plan_list(&v);
    assert_eq!(plans.len(), 2);
    assert_eq!(plans[1].prod_id, "sporty");
    assert_eq!(list_response_data_len(&v), 2);

    let v = json!({
        "status": "00",
        "data": {
            "CableTv": {
                "DSTV": [{"prodId": "dstv-padi", "prodName": "DStv Padi", "prodAmount": "4400.00"}],
                "GOtv": [{"prodId": "gotv-lite", "prodName": "GOtv Lite", "prodAmount": "900"}]
            }
        }
    });
    let plans = parse_cable_tv_list(&v);
    assert_eq!(plans.len(), 2);
    assert!(plans.iter().any(|p| p.prod_id == "dstv-padi"));

    let v = json!({
        "status": "00",
        "data": { "EKEDC": [{"prodId": "p1"}], "IKEDC": [{"prodId": "x"}], "discount": "0.02" }
    });
    let d = parse_elect_disco_directory(&v);
    assert!(d.iter().any(|x| x.prod_id == "EKEDC"));
    assert!(d.iter().any(|x| x.prod_id == "IKEDC"));
}

#[test]
fn airbills_elect_and_network_bucket_cases() {
    let v = json!({
        "status": "00",
        "data": {
            "EKEDC": [{"prodId": "p99", "prodName": "Prepaid", "prodAmount": 5000}],
            "IKEDC": [{"prodId": "x", "prodName": "Other", "prodAmount": 100}]
        }
    });
    let p = parse_elect_plans_for_disco(&v, "EKEDC", "01");
    assert_eq!(p.len(), 1);
    assert_eq!(p[0].prod_id, "p99");
    assert_eq!(p[0].batch.as_deref(), Some("01"));

    let v = json!({
        "status": "00",
        "data": {
            "batch": "01",
            "dataPlan": {
                "Airtel": [{"prodId": "499.91", "prodName": "1GB", "prodAmount": 530}],
                "MTN": [{"prodId": "1", "prodName": "500MB", "prodAmount": 100}]
            }
        }
    });
    let all = parse_internet_plan_list(&v);
    assert_eq!(all.len(), 2);
    let airtel = parse_internet_plan_list_for_network(&v, "AIRTEL");
    assert_eq!(airtel.len(), 1);
    assert_eq!(airtel[0].prod_id, "499.91");
}
