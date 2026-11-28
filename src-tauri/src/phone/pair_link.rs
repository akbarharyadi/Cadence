//! The pairing link a QR code carries: `cadence://pair?a=192.168.1.10:47300&c=482913&n=DESKTOP`.
//! The computer makes it (address, the open code, its name) and shows it as a QR; the phone's
//! camera app opens it in Cadence, which asks before pairing. A link can come from anywhere (a web
//! page, a message), so it's checked: only an address on the phone's own network counts, and the
//! code must be 6 digits.

use std::net::IpAddr;

use serde::Serialize;

use super::client::normalize_address;

const SCHEME: &str = "cadence://pair?";
/// The longest link read.
const MAX_LINK: usize = 300;

/// What a link offers.
#[derive(Clone, Debug, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Offer {
    pub address: String,
    pub code: String,
    pub computer: String,
}

/// The link for this computer's address and code.
pub fn make(address: &str, code: &str, computer: &str) -> String {
    format!("{SCHEME}a={}&c={}&n={}", urlencoding::encode(address), urlencoding::encode(code), urlencoding::encode(computer))
}

/// The link as a QR code (an SVG image), or None when it can't be drawn.
pub fn qr_svg(link: &str) -> Option<String> {
    let code = qrcode::QrCode::with_error_correction_level(link.as_bytes(), qrcode::EcLevel::M).ok()?;
    Some(code.render::<qrcode::render::svg::Color>().min_dimensions(220, 220).dark_color(qrcode::render::svg::Color("#000000")).light_color(qrcode::render::svg::Color("#ffffff")).build())
}

/// An address on a home network, USB tethering or this device: the pairing code is only ever sent
/// to such an address, so a link can't point the phone at a server on the internet.
fn local(address: &str) -> bool {
    let host = address.rsplit_once(':').map_or(address, |(h, _)| h);
    match host.parse::<IpAddr>() {
        Ok(IpAddr::V4(ip)) => ip.is_private() || ip.is_loopback() || ip.is_link_local(),
        Ok(IpAddr::V6(_)) => false,
        Err(_) => host.to_ascii_lowercase().ends_with(".local"),
    }
}

/// What a link offers, or why it isn't taken.
pub fn parse(link: &str) -> Result<Offer, String> {
    let bad = || "That isn't a Cadence pairing code.".to_string();
    let link = link.trim();
    if link.len() > MAX_LINK {
        return Err(bad());
    }
    let query = link.strip_prefix(SCHEME).ok_or_else(bad)?;
    let (mut address, mut code, mut computer) = (String::new(), String::new(), String::new());
    for pair in query.split('&') {
        let Some((k, v)) = pair.split_once('=') else { continue };
        let v = urlencoding::decode(v).map_err(|_| bad())?.into_owned();
        match k {
            "a" => address = v,
            "c" => code = v,
            "n" => computer = v.chars().filter(|c| !c.is_control()).take(60).collect(),
            _ => {}
        }
    }
    let address = normalize_address(&address)?;
    if !local(&address) {
        return Err("That pairing code points outside your own network, so it isn't used.".into());
    }
    if code.len() != 6 || !code.chars().all(|c| c.is_ascii_digit()) {
        return Err(bad());
    }
    Ok(Offer { address, code, computer })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_link_round_trips() {
        let link = make("192.168.1.10:47300", "482913", "Akbar's PC");
        assert_eq!(parse(&link), Ok(Offer { address: "192.168.1.10:47300".into(), code: "482913".into(), computer: "Akbar's PC".into() }));
    }

    #[test]
    fn only_addresses_on_the_phones_own_network_are_taken() {
        for ok in ["192.168.1.10:47300", "10.0.0.5:47301", "172.20.0.2:47300", "169.254.3.4:47300", "desk.local:47300"] {
            assert!(parse(&make(ok, "123456", "PC")).is_ok(), "{ok}");
        }
        for no in ["8.8.8.8:47300", "evil.example.com:47300", "172.32.0.1:47300", "[2001:db8::1]:47300"] {
            assert!(parse(&make(no, "123456", "PC")).is_err(), "{no}");
        }
    }

    #[test]
    fn a_bad_code_or_a_foreign_link_is_refused() {
        assert!(parse(&make("192.168.1.10:47300", "12345", "PC")).is_err());
        assert!(parse(&make("192.168.1.10:47300", "12a456", "PC")).is_err());
        assert!(parse("https://example.com/pair?a=192.168.1.10&c=123456").is_err());
        assert!(parse(&format!("{SCHEME}{}", "x".repeat(400))).is_err());
        assert!(parse("cadence://pair?c=123456").is_err(), "no address");
    }

    #[test]
    fn the_qr_is_an_svg_image() {
        let svg = qr_svg(&make("192.168.1.10:47300", "482913", "PC")).unwrap();
        assert!(svg.starts_with("<?xml") || svg.starts_with("<svg"));
        assert!(svg.contains("<svg") && svg.contains("</svg>"));
    }
}
