use super::*;

fn d(y: i32, m: u32, day: u32) -> NaiveDate {
    NaiveDate::from_ymd_opt(y, m, day).unwrap()
}

fn eur(s: &str) -> Money {
    Money::parse_de(s).unwrap()
}

fn ledger() -> Ledger {
    let mut l = Ledger::open_in_memory().unwrap();
    assert!(l.seed_chart().unwrap() > 30);
    l
}

fn line(account: u32, amount: &str, key: Option<TaxKey>) -> LineInput {
    LineInput {
        account,
        amount: eur(amount),
        tax_key: key,
        memo: None,
    }
}

/// Sales invoice: 1.200,00 gross receivable, 1.000,00 net revenue at 20%.
fn sale(l: &mut Ledger, date: NaiveDate) -> Entry {
    l.book(&EntryInput {
        date,
        kreis: Kreis::AR,
        beleg_nr: None,
        description: "Rechnung Beratung".into(),
        lines: vec![
            line(2000, "1.200,00", None),
            line(4000, "-1.000,00", Some(TaxKey::U20)),
        ],
    })
    .unwrap()
}

#[test]
fn engine_generates_tax_line_and_balances() {
    let mut l = ledger();
    let e = sale(&mut l, d(2026, 10, 5));
    assert_eq!(e.beleg_nr, "AR-2026-0001");
    assert_eq!(e.lines.len(), 3);
    let tax: Vec<&Line> = e.lines.iter().filter(|x| x.is_tax).collect();
    assert_eq!(tax.len(), 1);
    assert_eq!(tax[0].account, 3500);
    assert_eq!(tax[0].amount, eur("-200,00"));
    let sum = Money::sum(e.lines.iter().map(|x| x.amount)).unwrap();
    assert!(sum.is_zero());
    assert_eq!(sale(&mut l, d(2026, 10, 6)).beleg_nr, "AR-2026-0002");
}

#[test]
fn caller_cannot_double_count_vat() {
    // The old Gemini bug: caller books the tax line with a key too. Here the
    // engine would add a second tax line, so the entry no longer balances.
    let mut l = ledger();
    let err = l
        .book(&EntryInput {
            date: d(2026, 10, 5),
            kreis: Kreis::AR,
            beleg_nr: None,
            description: "double tax".into(),
            lines: vec![
                line(2000, "1.200,00", None),
                line(4000, "-1.000,00", Some(TaxKey::U20)),
                line(3500, "-200,00", None),
            ],
        })
        .unwrap_err();
    assert!(matches!(err, BuchError::Unbalanced { .. }), "{err}");
}

#[test]
fn rejects_bad_entries() {
    let mut l = ledger();
    let base = EntryInput {
        date: d(2026, 10, 5),
        kreis: Kreis::SO,
        beleg_nr: None,
        description: "x".into(),
        lines: vec![line(7600, "10,00", None), line(2700, "-9,99", None)],
    };
    assert!(matches!(
        l.book(&base).unwrap_err(),
        BuchError::Unbalanced { .. }
    ));
    let mut one = base.clone();
    one.lines.truncate(1);
    assert!(l.book(&one).is_err());
    let mut unknown = base.clone();
    unknown.lines = vec![line(1234, "1,00", None), line(2700, "-1,00", None)];
    assert!(matches!(
        l.book(&unknown).unwrap_err(),
        BuchError::NotFound(_)
    ));
    let mut zero = base.clone();
    zero.lines = vec![line(7600, "0,00", None), line(2700, "0,00", None)];
    assert!(l.book(&zero).is_err());
    let mut empty_desc = base;
    empty_desc.description = "  ".into();
    empty_desc.lines = vec![line(7600, "1,00", None), line(2700, "-1,00", None)];
    assert!(l.book(&empty_desc).is_err());
}

#[test]
fn input_tax_and_reverse_charge() {
    let mut l = ledger();
    let e = l
        .book(&EntryInput {
            date: d(2026, 10, 7),
            kreis: Kreis::ER,
            beleg_nr: Some("RE-4711".into()),
            description: "Bueromaterial".into(),
            lines: vec![
                line(7600, "50,00", Some(TaxKey::V20)),
                line(3300, "-60,00", None),
            ],
        })
        .unwrap();
    assert!(e
        .lines
        .iter()
        .any(|x| x.is_tax && x.account == 2500 && x.amount == eur("10,00")));
    let rc = l
        .book(&EntryInput {
            date: d(2026, 10, 8),
            kreis: Kreis::ER,
            beleg_nr: None,
            description: "Ware aus DE".into(),
            lines: vec![
                line(5000, "1.000,00", Some(TaxKey::IGE20)),
                line(3300, "-1.000,00", None),
            ],
        })
        .unwrap();
    assert_eq!(rc.lines.iter().filter(|x| x.is_tax).count(), 2);
}

#[test]
fn booked_rows_are_immutable_at_the_sql_level() {
    let mut l = ledger();
    let e = sale(&mut l, d(2026, 10, 5));
    assert!(l
        .conn
        .execute(
            "UPDATE lines SET amount_cents = 1 WHERE entry_id = ?1",
            params![e.id]
        )
        .is_err());
    assert!(l
        .conn
        .execute("DELETE FROM lines WHERE entry_id = ?1", params![e.id])
        .is_err());
    assert!(l
        .conn
        .execute("DELETE FROM entries WHERE id = ?1", params![e.id])
        .is_err());
    assert!(l
        .conn
        .execute(
            "UPDATE entries SET description = 'x' WHERE id = ?1",
            params![e.id]
        )
        .is_err());
}

#[test]
fn storno_links_both_ways_and_only_once() {
    let mut l = ledger();
    let e = sale(&mut l, d(2026, 10, 5));
    let st = l.reverse(e.id, d(2026, 10, 9), "falscher Betrag").unwrap();
    assert_eq!(st.reverses_id, Some(e.id));
    assert_eq!(st.beleg_nr, "ST-AR-2026-0001");
    assert_eq!(l.entry(e.id).unwrap().reversed_by_id, Some(st.id));
    let all = Money::sum(e.lines.iter().chain(st.lines.iter()).map(|x| x.amount)).unwrap();
    assert!(all.is_zero());
    assert!(l.reverse(e.id, d(2026, 10, 9), "again").is_err());
    assert!(l
        .reverse(st.id, d(2026, 10, 9), "storno of storno")
        .is_err());
    let e2 = sale(&mut l, d(2026, 10, 10));
    assert!(l.reverse(e2.id, d(2026, 10, 1), "before original").is_err());
    // A reversed sale contributes nothing to U30.
    let u = l.u30("2026-10").unwrap();
    assert_eq!(u.kz.get("022").copied(), Some(eur("1.000,00")));
}

#[test]
fn closed_periods_reject_bookings() {
    let mut l = ledger();
    let e = sale(&mut l, d(2026, 9, 30));
    l.close_period("2026-09").unwrap();
    assert!(matches!(
        l.book(&EntryInput {
            date: d(2026, 9, 15),
            kreis: Kreis::SO,
            beleg_nr: None,
            description: "late".into(),
            lines: vec![line(7600, "1,00", None), line(2700, "-1,00", None)],
        })
        .unwrap_err(),
        BuchError::PeriodClosed(_)
    ));
    // Storno must be dated in an open month.
    assert!(l.reverse(e.id, d(2026, 9, 30), "x").is_err());
    assert!(l.reverse(e.id, d(2026, 10, 1), "x").is_ok());
    assert!(l.close_period("2026-09").is_err());
    assert!(l.close_period("2026-13").is_err());
    assert_eq!(l.closed_periods().unwrap(), vec!["2026-09".to_string()]);
}

#[test]
fn u30_prefill_numbers() {
    let mut l = ledger();
    sale(&mut l, d(2026, 10, 5)); // 1.000 @20% -> 200 output tax
    l.book(&EntryInput {
        date: d(2026, 10, 6),
        kreis: Kreis::AR,
        beleg_nr: None,
        description: "Buch".into(),
        lines: vec![
            line(2000, "110,00", None),
            line(4010, "-100,00", Some(TaxKey::U10)),
        ],
    })
    .unwrap();
    l.book(&EntryInput {
        date: d(2026, 10, 7),
        kreis: Kreis::ER,
        beleg_nr: None,
        description: "Software".into(),
        lines: vec![
            line(7650, "300,00", Some(TaxKey::V20)),
            line(3300, "-360,00", None),
        ],
    })
    .unwrap();
    l.book(&EntryInput {
        date: d(2026, 10, 8),
        kreis: Kreis::ER,
        beleg_nr: None,
        description: "ig Erwerb".into(),
        lines: vec![
            line(5000, "500,00", Some(TaxKey::IGE20)),
            line(3300, "-500,00", None),
        ],
    })
    .unwrap();
    // Out of month: ignored.
    sale(&mut l, d(2026, 11, 1));
    let u = l.u30("2026-10").unwrap();
    let kz = |k: &str| u.kz.get(k).copied().unwrap_or(Money::ZERO);
    assert_eq!(kz("000"), eur("1.100,00"));
    assert_eq!(kz("022"), eur("1.000,00"));
    assert_eq!(kz("029"), eur("100,00"));
    assert_eq!(kz("060"), eur("60,00"));
    assert_eq!(kz("072"), eur("500,00"));
    assert_eq!(kz("070"), eur("500,00"));
    assert_eq!(kz("065"), eur("100,00"));
    // Output 200 + 10 + 100 (ig) = 310; input 60 + 100 = 160; payable 150.
    assert_eq!(u.output_tax, eur("310,00"));
    assert_eq!(u.input_tax, eur("160,00"));
    assert_eq!(kz("095"), eur("150,00"));
    assert!(l.u30("2026-1").is_err());
}

#[test]
fn balances_and_account_ledger() {
    let mut l = ledger();
    sale(&mut l, d(2026, 10, 5));
    sale(&mut l, d(2026, 10, 6));
    let b = l.balances(d(2026, 1, 1), d(2026, 12, 31)).unwrap();
    assert_eq!(b.total_debit, b.total_credit);
    let rev = b.rows.iter().find(|r| r.account == 4000).unwrap();
    assert_eq!(rev.saldo, eur("-2.000,00"));
    let (opening, rows) = l
        .account_ledger(2000, d(2026, 10, 6), d(2026, 10, 31))
        .unwrap();
    assert_eq!(opening, eur("1.200,00"));
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0].running, eur("2.400,00"));
    assert!(l
        .account_ledger(1234, d(2026, 1, 1), d(2026, 12, 31))
        .is_err());
}

#[test]
fn bank_import_dedupes_and_books() {
    let mut l = ledger();
    let tx = |amount: &str, r: &str| BankTxInput {
        booking_date: d(2026, 10, 12),
        value_date: None,
        partner: Some("A1 Telekom".into()),
        iban: Some("AT611904300234573201".into()),
        reference: r.into(),
        amount: eur(amount),
        currency: "EUR".into(),
    };
    // Two identical coffee-style lines on the same day are both real.
    let batch = vec![
        tx("-60,00", "Rechnung 10/2026"),
        tx("-3,50", "Kaffee"),
        tx("-3,50", "Kaffee"),
    ];
    let s = l.import_bank(&batch).unwrap();
    assert_eq!((s.imported, s.duplicates), (3, 0));
    let again = l.import_bank(&batch).unwrap();
    assert_eq!((again.imported, again.duplicates), (0, 3));
    let open = l.bank_unmatched(100).unwrap();
    assert_eq!(open.len(), 3);
    let tel = open.iter().find(|t| t.amount == eur("-60,00")).unwrap();
    let e = l.bank_book(tel.id, 7380, Some(TaxKey::V20), None).unwrap();
    assert_eq!(e.kreis, Kreis::BK);
    assert!(e
        .lines
        .iter()
        .any(|x| x.account == 7380 && x.amount == eur("50,00")));
    assert!(e
        .lines
        .iter()
        .any(|x| x.account == 2500 && x.amount == eur("10,00")));
    assert!(e
        .lines
        .iter()
        .any(|x| x.account == 2800 && x.amount == eur("-60,00")));
    assert!(l.bank_book(tel.id, 7380, None, None).is_err());
    assert_eq!(l.bank_unmatched(100).unwrap().len(), 2);
}

#[test]
fn drafts_from_bus_are_booked_only_on_approval() {
    let mut l = ledger();
    let input = EntryInput {
        date: d(2026, 12, 31),
        kreis: Kreis::SO,
        beleg_nr: None,
        description: "AfA 2026 GOL-001".into(),
        lines: vec![line(7000, "1.333,33", None), line(620, "-1.333,33", None)],
    };
    let id = l
        .add_draft("assetcraft", "asset.depreciation_due", Some("ev-1"), &input)
        .unwrap()
        .unwrap();
    assert!(l
        .add_draft("assetcraft", "asset.depreciation_due", Some("ev-1"), &input)
        .unwrap()
        .is_none());
    assert_eq!(l.stats().unwrap()["entries"], 0);
    let e = l.approve_draft(id).unwrap();
    assert_eq!(e.lines.len(), 2);
    assert!(l.approve_draft(id).is_err());
    assert!(l.discard_draft(id).is_err());
    assert_eq!(l.drafts(true).unwrap().len(), 0);
}

#[test]
fn gross_input_splits_correctly() {
    let i = Ledger::gross_input(
        d(2026, 10, 1),
        Kreis::KA,
        "Taxi",
        7600,
        2700,
        eur("11,00"),
        Some(TaxKey::V10),
        None,
    )
    .unwrap();
    assert_eq!(i.lines[0].amount, eur("10,00"));
    assert_eq!(i.lines[1].amount, eur("-11,00"));
    assert!(Ledger::gross_input(
        d(2026, 10, 1),
        Kreis::KA,
        "x",
        7600,
        2700,
        Money::ZERO,
        None,
        None
    )
    .is_err());
}

#[test]
fn reopens_file_database() {
    let path = std::env::temp_dir().join(format!("buchcraft-test-{}.db", std::process::id()));
    let _ = std::fs::remove_file(&path);
    {
        let mut l = Ledger::open(&path).unwrap();
        l.seed_chart().unwrap();
        sale(&mut l, d(2026, 10, 5));
    }
    let l = Ledger::open(&path).unwrap();
    assert_eq!(l.stats().unwrap()["entries"], 1);
    drop(l);
    for ext in ["", "-wal", "-shm"] {
        let _ = std::fs::remove_file(format!("{}{ext}", path.display()));
    }
}
