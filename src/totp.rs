// SPDX-License-Identifier: EUPL-1.2
// Copyright (c) 2026 example.org

//! Zwei-Faktor-Anmeldung mit Einmal-Codes aus einer App (TOTP, RFC 6238):
//! HMAC-SHA1, 6 Ziffern, 30 Sekunden – das Verfahren von Google Authenticator,
//! Microsoft Authenticator, FreeOTP, Aegis usw.

use hmac::{Hmac, KeyInit, Mac};
use sha1::Sha1;

/// Name in der App. Bei einer Umbenennung hier anpassen (bestehende Einträge in den
/// Apps behalten den alten Namen, funktionieren aber weiter).
pub const ISSUER: &str = "Mimi";
const STEP: u64 = 30;
const DIGITS: u32 = 6;
/// Toleranz für Uhrabweichung: ein Zeitschritt davor und danach
const WINDOW: i64 = 1;

const B32: &[u8; 32] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZ234567";

pub fn base32_encode(data: &[u8]) -> String {
    let mut out = String::new();
    let mut buf: u64 = 0;
    let mut bits = 0;
    for &b in data {
        buf = (buf << 8) | b as u64;
        bits += 8;
        while bits >= 5 {
            bits -= 5;
            out.push(B32[((buf >> bits) & 31) as usize] as char);
        }
    }
    if bits > 0 {
        out.push(B32[((buf << (5 - bits)) & 31) as usize] as char);
    }
    out
}

pub fn base32_decode(s: &str) -> Option<Vec<u8>> {
    let mut out = Vec::new();
    let mut buf: u64 = 0;
    let mut bits = 0;
    for c in s.chars().filter(|c| !c.is_whitespace() && *c != '=' && *c != '-') {
        let v = B32.iter().position(|&x| x as char == c.to_ascii_uppercase())? as u64;
        buf = (buf << 5) | v;
        bits += 5;
        if bits >= 8 {
            bits -= 8;
            out.push(((buf >> bits) & 0xff) as u8);
        }
    }
    Some(out)
}

/// Neues Geheimnis: 20 Zufallsbytes (160 Bit), Base32.
pub fn new_secret() -> String {
    let mut b = [0u8; 20];
    getrandom::fill(&mut b).expect("Zufallszahlen nicht verfügbar");
    base32_encode(&b)
}

fn hotp(key: &[u8], counter: u64, digits: u32) -> u32 {
    let mut mac = <Hmac<Sha1> as KeyInit>::new_from_slice(key).expect("HMAC nimmt jede Schlüssellänge");
    mac.update(&counter.to_be_bytes());
    let h = mac.finalize().into_bytes();
    let off = (h[h.len() - 1] & 0x0f) as usize;
    let bin = ((h[off] as u32 & 0x7f) << 24) | ((h[off + 1] as u32) << 16) | ((h[off + 2] as u32) << 8) | h[off + 3] as u32;
    bin % 10u32.pow(digits)
}

fn now_secs() -> u64 {
    std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_secs()).unwrap_or(0)
}

/// Prüft einen Code. Gibt den Zeitschritt zurück, damit derselbe Code nicht zweimal gilt.
pub fn verify(secret: &str, code: &str) -> Option<u64> {
    verify_at(secret, code, now_secs())
}

fn verify_at(secret: &str, code: &str, now: u64) -> Option<u64> {
    let code: String = code.chars().filter(|c| !c.is_whitespace()).collect();
    if code.len() != DIGITS as usize || !code.chars().all(|c| c.is_ascii_digit()) {
        return None;
    }
    let want: u32 = code.parse().ok()?;
    let key = base32_decode(secret)?;
    let step = (now / STEP) as i64;
    (-WINDOW..=WINDOW).map(|d| step + d).filter(|s| *s >= 0).find(|s| hotp(&key, *s as u64, DIGITS) == want).map(|s| s as u64)
}

fn pct(s: &str) -> String {
    s.bytes().map(|b| if b.is_ascii_alphanumeric() || b"-._~".contains(&b) { (b as char).to_string() } else { format!("%{b:02X}") }).collect()
}

/// Adresse für die App (steckt im QR-Code).
pub fn uri(account: &str, secret: &str) -> String {
    format!("otpauth://totp/{}:{}?secret={secret}&issuer={}&algorithm=SHA1&digits={DIGITS}&period={STEP}", pct(ISSUER), pct(account), pct(ISSUER))
}

/// QR-Code als SVG zum Einbetten in die Seite.
pub fn qr_svg(data: &str) -> String {
    match qrcode::QrCode::new(data.as_bytes()) {
        Ok(q) => q.render::<qrcode::render::svg::Color>().min_dimensions(200, 200).quiet_zone(true).build(),
        Err(_) => String::new(),
    }
}

/// Geheimnis in Vierergruppen zum Abtippen.
pub fn grouped(secret: &str) -> String {
    secret.as_bytes().chunks(4).map(|c| String::from_utf8_lossy(c).into_owned()).collect::<Vec<_>>().join(" ")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rfc6238_vectors() {
        // RFC 6238, Anhang B (SHA1, Schlüssel „12345678901234567890“), auf 6 Ziffern gekürzt
        let key = b"12345678901234567890";
        let secret = base32_encode(key);
        for (t, code) in [(59u64, "287082"), (1111111109, "081804"), (1234567890, "005924"), (2000000000, "279037")] {
            assert_eq!(verify_at(&secret, code, t), Some(t / 30), "t={t}");
        }
        // 8 Ziffern wie im RFC
        assert_eq!(hotp(key, 59 / 30, 8), 94287082);
    }

    #[test]
    fn window_and_rejects() {
        let secret = new_secret();
        let key = base32_decode(&secret).unwrap();
        let now = 1_800_000_000;
        let code = |step: u64| format!("{:06}", hotp(&key, step, 6));
        let s = now / 30;
        assert_eq!(verify_at(&secret, &code(s), now), Some(s));
        assert_eq!(verify_at(&secret, &code(s - 1), now), Some(s - 1));
        assert_eq!(verify_at(&secret, &code(s + 1), now), Some(s + 1));
        assert_eq!(verify_at(&secret, &code(s - 3), now), None);
        assert_eq!(verify_at(&secret, "12345", now), None);
        assert_eq!(verify_at(&secret, "abcdef", now), None);
        // Leerzeichen in der Eingabe sind erlaubt
        let c = code(s);
        assert_eq!(verify_at(&secret, &format!("{} {}", &c[..3], &c[3..]), now), Some(s));
    }

    #[test]
    fn base32_roundtrip() {
        for data in [&b""[..], b"f", b"fo", b"foo", b"foob", b"fooba", b"foobar"] {
            assert_eq!(base32_decode(&base32_encode(data)).unwrap(), data);
        }
        assert_eq!(base32_encode(b"foobar"), "MZXW6YTBOI");
        assert_eq!(new_secret().len(), 32);
    }

    #[test]
    fn uri_and_qr() {
        let u = uri("anna maier", "JBSWY3DPEHPK3PXP");
        assert!(u.starts_with("otpauth://totp/Mimi:anna%20maier?secret=JBSWY3DPEHPK3PXP&issuer=Mimi"));
        assert!(qr_svg(&u).contains("<svg"));
        assert_eq!(grouped("JBSWY3DPEHPK3PXP"), "JBSW Y3DP EHPK 3PXP");
    }
}
