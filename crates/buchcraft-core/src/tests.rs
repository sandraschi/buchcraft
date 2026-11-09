use super::*;

fn ctx(tag: &str) -> Ctx {
    let bus_root = std::env::temp_dir().join(format!("buchcraft-bus-{tag}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&bus_root);
    Ctx {
        ledger: Ledger::open_in_memory().unwrap(),
        confinement: Confinement(None),
        bus: Bus::new(bus_root),
    }
}

fn call(c: &mut Ctx, name: &str, args: Value) -> Result<Value, String> {
    let cmd = Command::from_tool(name, &args)?;
    dispatch(c, cmd)
}

#[test]
fn every_catalog_tool_is_routable() {
    // Each tool name in the catalog must be known to from_tool (arguments may be missing).
    for t in tools() {
        let name = t["name"].as_str().unwrap();
        let r = Command::from_tool(name, &json!({}));
        if let Err(e) = r {
            assert!(!e.starts_with("unknown tool"), "{name} not routable");
        }
        assert!(name.starts_with("buch_"));
    }
    assert!(Command::from_tool("buch_nope", &json!({}))
        .unwrap_err()
        .starts_with("unknown tool"));
}

#[test]
fn demo_seed_then_reports() {
    let mut c = ctx("demo");
    let seeded = call(&mut c, "buch_demo_seed", json!({})).unwrap();
    assert_eq!(seeded["demo"], true);
    assert!(
        call(&mut c, "buch_demo_seed", json!({})).is_err(),
        "demo must refuse a non-empty ledger"
    );
    let status = call(&mut c, "buch.status", json!({})).unwrap();
    assert_eq!(status["stats"]["demo"], true);
    assert_eq!(status["stats"]["bank_unmatched"], 2);
    let u30 = call(&mut c, "buch_u30", json!({ "month": "2026-10" })).unwrap();
    // Sales 3.000 + 1.000 net (the 1.100 one is reversed) -> KZ 022 = 4.000,00
    assert_eq!(u30["kz_de"]["022"], "4.000,00");
    // Output 800 + ig 40 = 840; input 250 + 8 + 10 (A1) + 40 (ig) = 308; payable 532
    assert_eq!(u30["kz_de"]["095"], "532,00");
    let bal = call(
        &mut c,
        "buch_balance",
        json!({ "from": "01.10.2026", "to": "31.10.2026" }),
    )
    .unwrap();
    assert_eq!(bal["total_debit"], bal["total_credit"]);
}

#[test]
fn book_via_tool_with_de_strings_and_cents() {
    let mut c = ctx("book");
    call(&mut c, "buch_accounts_seed", json!({})).unwrap();
    let e = call(
        &mut c,
        "buch_entry_book",
        json!({
            "date": "05.10.2026", "kreis": "ar", "description": "Rechnung",
            "lines": [
                { "account": 2000, "amount": "1.200,00" },
                { "account": 4000, "amount_cents": -100000, "tax_key": "U20" }
            ]
        }),
    )
    .unwrap();
    assert_eq!(e["lines"].as_array().map(Vec::len), Some(3));
    // Floats are refused: 0.1 + 0.2 is not money.
    let float = call(
        &mut c,
        "buch_entry_book",
        json!({ "date": "05.10.2026", "kreis": "AR", "description": "x",
                "lines": [{ "account": 2000, "amount": 1.5 }, { "account": 4000, "amount": -1.5 }] }),
    );
    assert!(float.unwrap_err().contains("not a float"));
    assert!(call(
        &mut c,
        "buch_entry_book",
        json!({ "date": "05.10.2026", "kreis": "XX", "description": "x", "lines": [] })
    )
    .is_err());
    assert!(call(&mut c, "buch_entry_show", json!({ "id": 999 }))
        .unwrap_err()
        .contains("not found"));
    let gross = call(
        &mut c,
        "buch_entry_book_gross",
        json!({ "date": "2026-10-09", "kreis": "KA", "description": "Taxi", "account": 7600, "contra": 2700, "gross": "-11,00", "tax_key": "V10" }),
    );
    // Negative gross on an expense account would be a credit note; allowed and balanced.
    assert!(gross.is_ok());
}

#[test]
fn inbox_drain_creates_drafts_and_rejects_unknown() {
    let mut c = ctx("inbox");
    call(&mut c, "buch_accounts_seed", json!({})).unwrap();
    c.bus
        .emit("assetcraft", APP, "asset.depreciation_due", json!({ "asset_tag": "GOL-001", "year": 2026, "amount_cents": 133_333, "debit": 7000, "credit": 620 }))
        .unwrap();
    c.bus
        .emit("assetcraft", APP, "something.else", json!({}))
        .unwrap();
    c.bus.emit("assetcraft", APP, "asset.depreciation_due", json!({ "asset_tag": "X", "year": 2026, "amount_cents": -5, "debit": 7000, "credit": 620 })).unwrap();
    let r = call(&mut c, "buch_inbox_drain", json!({})).unwrap();
    assert_eq!(r["drafted"].as_array().map(Vec::len), Some(1));
    assert_eq!(r["rejected"].as_array().map(Vec::len), Some(2));
    let drafts = call(&mut c, "buch_drafts", json!({})).unwrap();
    let id = drafts["drafts"][0]["id"].as_i64().unwrap();
    let e = call(&mut c, "buch_draft_approve", json!({ "id": id })).unwrap();
    assert_eq!(e["date"], "2026-12-31");
    assert_eq!(
        call(&mut c, "buch_inbox_drain", json!({})).unwrap()["drafted"]
            .as_array()
            .map(Vec::len),
        Some(0)
    );
    let _ = std::fs::remove_dir_all(c.bus.root());
}

#[test]
fn confinement_blocks_files_outside_roots() {
    let root = std::env::temp_dir().join(format!("buchcraft-conf-{}", std::process::id()));
    std::fs::create_dir_all(&root).unwrap();
    let csv = root.join("bank.csv");
    std::fs::write(
        &csv,
        "Buchungsdatum;Betrag;Verwendungszweck\n12.10.2026;-60,00;A1\n",
    )
    .unwrap();
    let mut c = ctx("conf");
    call(&mut c, "buch_accounts_seed", json!({})).unwrap();
    c.confinement = Confinement(Some(Vec::new()));
    assert!(call(
        &mut c,
        "buch_bank_import",
        json!({ "path": csv.display().to_string() })
    )
    .unwrap_err()
    .contains("--allow-path"));
    c.confinement = Confinement::roots(std::slice::from_ref(&root)).unwrap();
    let r = call(
        &mut c,
        "buch_bank_import",
        json!({ "path": csv.display().to_string() }),
    )
    .unwrap();
    assert_eq!(r["imported"], 1);
    let outside = std::env::temp_dir().join("buchcraft-outside.csv");
    assert!(call(
        &mut c,
        "buch_export_csv",
        json!({ "from": "01.10.2026", "to": "31.10.2026", "path": outside.display().to_string() })
    )
    .unwrap_err()
    .contains("outside"));
    let inside = root.join("journal.csv");
    let ex = call(
        &mut c,
        "buch_export_csv",
        json!({ "from": "01.10.2026", "to": "31.10.2026", "path": inside.display().to_string() }),
    )
    .unwrap();
    assert_eq!(ex["lines"], 0);
    assert!(inside.exists());
    let _ = std::fs::remove_dir_all(&root);
}
