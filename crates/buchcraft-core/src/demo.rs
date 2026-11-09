//! Demo company for first paint and recipes. Only fills an EMPTY ledger and
//! marks it `demo = 1` so every status output says so.
//!
//! "Joe Mocky Consulting e.U.", Vienna, October 2026.

use buchcraft_store::{BankTxInput, EntryInput, Kreis, Ledger, LineInput};
use chrono::NaiveDate;
use craft_biz::ekr::TaxKey;
use craft_biz::Money;
use serde_json::{json, Value};

fn d(day: u32) -> Result<NaiveDate, String> {
    NaiveDate::from_ymd_opt(2026, 10, day).ok_or_else(|| format!("bad demo day {day}"))
}

fn m(s: &str) -> Result<Money, String> {
    Money::parse_de(s).map_err(|e| e.to_string())
}

fn line(account: u32, amount: &str, tax_key: Option<TaxKey>) -> Result<LineInput, String> {
    Ok(LineInput {
        account,
        amount: m(amount)?,
        tax_key,
        memo: None,
    })
}

pub fn seed(l: &mut Ledger) -> Result<Value, String> {
    let stats = l.stats().map_err(|e| e.to_string())?;
    if stats["entries"].as_i64().unwrap_or(1) != 0 {
        return Err("demo seed only runs on an empty ledger".into());
    }
    l.seed_chart().map_err(|e| e.to_string())?;
    l.set_meta("demo", "1").map_err(|e| e.to_string())?;
    l.set_meta("company", "Joe Mocky Consulting e.U. (DEMO)")
        .map_err(|e| e.to_string())?;

    let mut book =
        |date: NaiveDate, kreis: Kreis, desc: &str, lines: Vec<LineInput>| -> Result<i64, String> {
            l.book(&EntryInput {
                date,
                kreis,
                beleg_nr: None,
                description: desc.into(),
                lines,
            })
            .map(|e| e.id)
            .map_err(|e| e.to_string())
        };
    book(
        d(1)?,
        Kreis::SO,
        "Eröffnung Kapital",
        vec![
            line(2800, "10.000,00", None)?,
            line(2700, "500,00", None)?,
            line(9000, "-10.500,00", None)?,
        ],
    )?;
    book(
        d(5)?,
        Kreis::AR,
        "Beratung Hannes Mockinger GmbH",
        vec![
            line(2000, "3.600,00", None)?,
            line(4000, "-3.000,00", Some(TaxKey::U20))?,
        ],
    )?;
    let wrong = book(
        d(6)?,
        Kreis::AR,
        "Workshop Mocky AG (falscher Betrag)",
        vec![
            line(2000, "1.320,00", None)?,
            line(4000, "-1.100,00", Some(TaxKey::U20))?,
        ],
    )?;
    book(
        d(7)?,
        Kreis::AR,
        "Workshop Mocky AG",
        vec![
            line(2000, "1.200,00", None)?,
            line(4000, "-1.000,00", Some(TaxKey::U20))?,
        ],
    )?;
    book(
        d(8)?,
        Kreis::ER,
        "Laptop",
        vec![
            line(620, "1.250,00", Some(TaxKey::V20))?,
            line(3300, "-1.500,00", None)?,
        ],
    )?;
    book(
        d(9)?,
        Kreis::ER,
        "Fachbücher (10%)",
        vec![
            line(7600, "80,00", Some(TaxKey::V10))?,
            line(2700, "-88,00", None)?,
        ],
    )?;
    book(
        d(12)?,
        Kreis::ER,
        "Software-Abo aus DE (ig. Erwerb)",
        vec![
            line(7650, "200,00", Some(TaxKey::IGE20))?,
            line(3300, "-200,00", None)?,
        ],
    )?;
    l.reverse(wrong, d(10)?, "falscher Betrag")
        .map_err(|e| e.to_string())?;

    let bank =
        |day: u32, partner: &str, reference: &str, amount: &str| -> Result<BankTxInput, String> {
            Ok(BankTxInput {
                booking_date: d(day)?,
                value_date: None,
                partner: Some(partner.into()),
                iban: None,
                reference: reference.into(),
                amount: m(amount)?,
                currency: "EUR".into(),
            })
        };
    let imported = l
        .import_bank(&[
            bank(15, "Hannes Mockinger GmbH", "AR-2026-0001", "3.600,00")?,
            bank(16, "A1 Telekom Austria AG", "Rechnung 10/2026", "-60,00")?,
            bank(20, "Wiener Linien", "Jahreskarte", "-365,00")?,
            bank(21, "Cafe Mocca", "Kartenzahlung", "-4,20")?,
        ])
        .map_err(|e| e.to_string())?;
    // Book the two obvious ones; leave the rest for the agent/human to classify.
    let open = l.bank_unmatched(10).map_err(|e| e.to_string())?;
    for t in &open {
        if t.reference == "AR-2026-0001" {
            l.bank_book(t.id, 2000, None, Some("Zahlung AR-2026-0001"))
                .map_err(|e| e.to_string())?;
        } else if t.reference == "Rechnung 10/2026" {
            l.bank_book(t.id, 7380, Some(TaxKey::V20), None)
                .map_err(|e| e.to_string())?;
        }
    }
    Ok(
        json!({ "demo": true, "company": "Joe Mocky Consulting e.U. (DEMO)", "bank_imported": imported.imported, "stats": l.stats().map_err(|e| e.to_string())? }),
    )
}
