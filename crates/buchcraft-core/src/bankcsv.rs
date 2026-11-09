//! Bank statement CSV import with header-name mapping.
//!
//! Austrian online-banking exports (George / Erste, Raiffeisen, Bank Austria)
//! differ in column names, delimiter and encoding. Rather than hard-code one
//! layout, columns are found by known German and English header aliases.
//! The alias list has NOT yet been verified against a real George export;
//! when a real file fails, add its header names here with a test.

use buchcraft_store::BankTxInput;
use craft_biz::at::parse_date;
use craft_biz::Money;

pub const MAX_FILE_BYTES: usize = 20 * 1024 * 1024;
pub const MAX_ROWS: usize = 100_000;

const DATE: &[&str] = &[
    "buchungsdatum",
    "booking date",
    "bookingdate",
    "datum",
    "date",
];
const VALUE: &[&str] = &["valutadatum", "valuta", "value date", "valuedate"];
const PARTNER: &[&str] = &[
    "partnername",
    "partner name",
    "empfaenger",
    "empf\u{e4}nger",
    "auftraggeber",
    "partner",
    "name",
];
const IBAN: &[&str] = &[
    "partner iban",
    "partner-iban",
    "partneriban",
    "iban",
    "partnerkonto",
    "partner account",
];
const REFERENCE: &[&str] = &[
    "verwendungszweck",
    "zahlungsreferenz",
    "buchungstext",
    "beschreibung",
    "details",
    "text",
    "reference",
];
const AMOUNT: &[&str] = &["betrag", "amount"];
const CURRENCY: &[&str] = &["w\u{e4}hrung", "waehrung", "currency"];

/// UTF-8 (BOM stripped), else Windows-1252 approximated as Latin-1 with the euro sign mapped.
fn decode(bytes: &[u8]) -> String {
    let bytes = bytes.strip_prefix(&[0xEF, 0xBB, 0xBF][..]).unwrap_or(bytes);
    match std::str::from_utf8(bytes) {
        Ok(s) => s.to_string(),
        Err(_) => bytes
            .iter()
            .map(|&b| if b == 0x80 { '\u{20ac}' } else { char::from(b) })
            .collect(),
    }
}

fn find(headers: &[String], aliases: &[&str]) -> Option<usize> {
    aliases
        .iter()
        .find_map(|a| headers.iter().position(|h| h == a))
}

fn parse_amount(s: &str) -> Result<Money, String> {
    Money::parse_de(s).or_else(|_| {
        // "-60.00" style: dot decimal with exactly two decimals and no comma.
        let t = s.trim();
        match t.rsplit_once('.') {
            Some((int, frac)) if !t.contains(',') && frac.len() == 2 && !int.contains('.') => {
                Money::parse_de(&format!("{int},{frac}")).map_err(|e| e.to_string())
            }
            _ => Err(format!("not an amount: {s:?}")),
        }
    })
}

pub fn parse_bank_csv(bytes: &[u8]) -> Result<Vec<BankTxInput>, String> {
    if bytes.len() > MAX_FILE_BYTES {
        return Err(format!(
            "file larger than {} MB",
            MAX_FILE_BYTES / 1024 / 1024
        ));
    }
    let text = decode(bytes);
    let first = text.lines().next().ok_or("empty file")?;
    let delim = if first.matches(';').count() >= first.matches(',').count() {
        b';'
    } else {
        b','
    };
    let mut rdr = csv::ReaderBuilder::new()
        .delimiter(delim)
        .flexible(true)
        .from_reader(text.as_bytes());
    let headers: Vec<String> = rdr
        .headers()
        .map_err(|e| e.to_string())?
        .iter()
        .map(|h| h.trim().trim_matches('"').to_lowercase())
        .collect();
    let date = find(&headers, DATE).ok_or("no booking-date column (Buchungsdatum/Datum)")?;
    let amount = find(&headers, AMOUNT).ok_or("no amount column (Betrag)")?;
    let value = find(&headers, VALUE);
    let partner = find(&headers, PARTNER);
    let iban = find(&headers, IBAN);
    let reference = find(&headers, REFERENCE);
    let currency = find(&headers, CURRENCY);
    if reference.is_none() && partner.is_none() {
        return Err("need a reference (Verwendungszweck/Buchungstext) or partner column".into());
    }
    let mut out = Vec::new();
    for (i, rec) in rdr.records().enumerate() {
        if i >= MAX_ROWS {
            return Err(format!("more than {MAX_ROWS} rows"));
        }
        let rec = rec.map_err(|e| format!("row {}: {e}", i + 2))?;
        let get = |c: Option<usize>| {
            c.and_then(|c| rec.get(c))
                .map(str::trim)
                .filter(|s| !s.is_empty())
                .map(str::to_string)
        };
        let Some(date_s) = get(Some(date)) else {
            continue;
        };
        let booking_date = parse_date(&date_s).map_err(|e| format!("row {}: {e}", i + 2))?;
        let amount_s = get(Some(amount)).ok_or_else(|| format!("row {}: empty amount", i + 2))?;
        let amount = parse_amount(&amount_s).map_err(|e| format!("row {}: {e}", i + 2))?;
        let partner_v = get(partner);
        let reference_v = get(reference)
            .or_else(|| partner_v.clone())
            .unwrap_or_else(|| "(ohne Text)".into());
        out.push(BankTxInput {
            booking_date,
            value_date: get(value).and_then(|v| parse_date(&v).ok()),
            partner: partner_v,
            iban: get(iban).map(|s| s.replace(' ', "")),
            reference: reference_v.chars().take(500).collect(),
            amount,
            currency: get(currency).unwrap_or_else(|| "EUR".into()).to_uppercase(),
        });
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_semicolon_german_export() {
        let csv = "Buchungsdatum;Valutadatum;Partnername;Partner IBAN;Verwendungszweck;Betrag;W\u{e4}hrung\n\
                   12.10.2026;12.10.2026;A1 Telekom;AT61 1904 3002 3457 3201;Rechnung 10/2026;-60,00;EUR\n\
                   13.10.2026;13.10.2026;Kunde GmbH;;Honorar AR-2026-0001;1.200,00;EUR\n";
        let rows = parse_bank_csv(csv.as_bytes()).unwrap();
        assert_eq!(rows.len(), 2);
        assert_eq!(rows[0].amount.cents(), -6000);
        assert_eq!(rows[0].iban.as_deref(), Some("AT611904300234573201"));
        assert_eq!(rows[1].amount.cents(), 120_000);
        assert_eq!(rows[1].iban, None);
    }

    #[test]
    fn parses_comma_english_with_dot_decimals_and_latin1() {
        let mut bytes =
            b"Date,Name,Details,Amount,Currency\n2026-10-12,Caf\xe9,Coffee,-3.50,EUR\n".to_vec();
        bytes.extend_from_slice(b"2026-10-13,Shop,\x80 deal,-10.00,EUR\n");
        let rows = parse_bank_csv(&bytes).unwrap();
        assert_eq!(rows.len(), 2);
        assert_eq!(rows[0].partner.as_deref(), Some("Caf\u{e9}"));
        assert_eq!(rows[0].amount.cents(), -350);
        assert!(rows[1].reference.starts_with('\u{20ac}'));
    }

    #[test]
    fn rejects_unusable_files() {
        assert!(parse_bank_csv(b"").is_err());
        assert!(parse_bank_csv(b"Foo;Bar\n1;2\n").is_err());
        assert!(parse_bank_csv("Datum;Betrag;Text\n32.13.2026;1,00;x\n".as_bytes()).is_err());
        assert!(parse_bank_csv("Datum;Betrag;Text\n01.10.2026;abc;x\n".as_bytes()).is_err());
    }
}
