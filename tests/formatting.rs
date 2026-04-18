use payce_ng::services::exchange_rate::{
    format_ngn, format_sol, format_stable_amount_with_code, format_stable_qty_trimmed, format_usdc,
};
use payce_ng::services::utility_bill::network_name_to_id;
use payce_ng::utils::phone::phone_local_nigeria_11_digits;

#[test]
fn format_ngn_commas() {
    assert_eq!(format_ngn(2000.0), "₦2,000");
    assert_eq!(format_ngn(1234567.0), "₦1,234,567");
    assert_eq!(format_ngn(1999.5), "₦1,999.50");
}

#[test]
fn format_usdc_commas() {
    assert_eq!(format_usdc(2000.0), "$2,000.00");
    assert_eq!(format_usdc(12.5), "$12.50");
}

#[test]
fn format_stable_qty_trimmed_two_dp_no_dollar() {
    assert_eq!(format_stable_qty_trimmed(1.49), "1.49");
    assert_eq!(format_stable_qty_trimmed(2.0), "2.00");
    assert_eq!(format_stable_qty_trimmed(1.4567), "1.46");
    assert_eq!(format_stable_qty_trimmed(1234.5), "1,234.50");
    assert_eq!(format_stable_qty_trimmed(0.000001), "0.00");
}

#[test]
fn format_stable_amount_with_code_for_ussd() {
    assert_eq!(format_stable_amount_with_code(1.49, "USDG"), "≈1.49 USDG");
}

#[test]
fn format_sol_commas() {
    assert_eq!(format_sol(2.0), "2 SOL");
    assert_eq!(format_sol(0.5), "0.5 SOL");
    assert_eq!(format_sol(1234.56789), "1,234.56789 SOL");
}

#[test]
fn network_name_maps() {
    assert_eq!(network_name_to_id("MTN"), "01");
    assert_eq!(network_name_to_id("glo"), "02");
    assert_eq!(network_name_to_id("Airtel"), "04");
}

#[test]
fn phone_local_11_from_country_code() {
    assert_eq!(
        phone_local_nigeria_11_digits("+2347068827272"),
        "07068827272"
    );
}
