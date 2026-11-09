//! buchcraft-store: the ledger.
//!
//! Invariants enforced here (and by SQLite triggers as a second wall):
//! - Every entry balances: the sum of its lines, including generated tax
//!   lines, is exactly zero cents.
//! - Callers book net lines with a tax key; the engine generates tax lines.
//!   A caller can never book a tax line directly, so VAT cannot be counted twice.
//! - Booked entries and lines are immutable. Corrections are Storno entries
//!   (`reverse`) that link both ways.
//! - Closed months reject bookings and reversals dated inside them.
#![deny(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use chrono::{Datelike, NaiveDate, Utc};
use craft_biz::ekr::{seed_accounts, AccountKind, TaxDirection, TaxKey};
use craft_biz::{BizError, Money};
use rusqlite::{params, Connection, OptionalExtension, Transaction};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::collections::{BTreeMap, HashMap};
use std::path::Path;

pub const SCHEMA_VERSION: i64 = 1;
const MAX_LINES: usize = 200;
const MAX_TEXT: usize = 500;

#[derive(Debug, thiserror::Error)]
pub enum BuchError {
    #[error(transparent)]
    Biz(#[from] BizError),
    #[error("database: {0}")]
    Db(String),
    #[error("invalid: {0}")]
    Invalid(String),
    #[error("not found: {0}")]
    NotFound(String),
    #[error("entry does not balance: debit {debit}, credit {credit}")]
    Unbalanced { debit: String, credit: String },
    #[error("period {0} is closed")]
    PeriodClosed(String),
}

impl From<rusqlite::Error> for BuchError {
    fn from(e: rusqlite::Error) -> Self {
        BuchError::Db(e.to_string())
    }
}

impl From<serde_json::Error> for BuchError {
    fn from(e: serde_json::Error) -> Self {
        BuchError::Invalid(e.to_string())
    }
}

pub type Result<T> = std::result::Result<T, BuchError>;

/// Belegkreis (document cycle).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum Kreis {
    /// Ausgangsrechnungen (sales invoices)
    AR,
    /// Eingangsrechnungen (purchase invoices)
    ER,
    /// Bank
    BK,
    /// Kassa (cash)
    KA,
    /// Sonstige (adjustments, depreciation, closing)
    SO,
}

impl Kreis {
    pub fn as_str(self) -> &'static str {
        match self {
            Kreis::AR => "AR",
            Kreis::ER => "ER",
            Kreis::BK => "BK",
            Kreis::KA => "KA",
            Kreis::SO => "SO",
        }
    }

    pub fn parse(s: &str) -> Result<Kreis> {
        match s.trim().to_ascii_uppercase().as_str() {
            "AR" => Ok(Kreis::AR),
            "ER" => Ok(Kreis::ER),
            "BK" => Ok(Kreis::BK),
            "KA" => Ok(Kreis::KA),
            "SO" => Ok(Kreis::SO),
            other => Err(BuchError::Invalid(format!(
                "unknown Belegkreis {other:?} (AR ER BK KA SO)"
            ))),
        }
    }
}

/// A line as the caller books it. `amount` is signed: positive = Soll (debit),
/// negative = Haben (credit). With a tax key, `amount` is the NET amount.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct LineInput {
    pub account: u32,
    pub amount: Money,
    #[serde(default)]
    pub tax_key: Option<TaxKey>,
    #[serde(default)]
    pub memo: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct EntryInput {
    pub date: NaiveDate,
    pub kreis: Kreis,
    #[serde(default)]
    pub beleg_nr: Option<String>,
    pub description: String,
    pub lines: Vec<LineInput>,
}

#[derive(Debug, Clone, Serialize)]
pub struct Account {
    pub number: u32,
    pub name: String,
    pub kind: AccountKind,
    pub default_tax_key: Option<TaxKey>,
    pub active: bool,
}

#[derive(Debug, Clone, Serialize)]
pub struct Line {
    pub id: i64,
    pub account: u32,
    pub account_name: String,
    pub amount: Money,
    pub amount_de: String,
    pub tax_key: Option<String>,
    pub is_tax: bool,
    pub memo: Option<String>,
}

#[derive(Debug, Clone, Serialize)]
pub struct Entry {
    pub id: i64,
    pub date: NaiveDate,
    pub kreis: Kreis,
    pub beleg_nr: String,
    pub description: String,
    pub created_at: String,
    pub reverses_id: Option<i64>,
    pub reversed_by_id: Option<i64>,
    pub lines: Vec<Line>,
}

#[derive(Debug, Clone, Serialize)]
pub struct BalanceRow {
    pub account: u32,
    pub name: String,
    pub kind: AccountKind,
    pub debit: Money,
    pub credit: Money,
    pub saldo: Money,
    pub saldo_de: String,
}

#[derive(Debug, Clone, Serialize)]
pub struct Balances {
    pub from: NaiveDate,
    pub to: NaiveDate,
    pub rows: Vec<BalanceRow>,
    pub total_debit: Money,
    pub total_credit: Money,
    pub total_debit_de: String,
    pub total_credit_de: String,
}

#[derive(Debug, Clone, Serialize)]
pub struct LedgerRow {
    pub entry_id: i64,
    pub date: NaiveDate,
    pub beleg_nr: String,
    pub description: String,
    pub amount: Money,
    pub running: Money,
    pub running_de: String,
}

#[derive(Debug, Clone, Serialize)]
pub struct U30Report {
    pub month: String,
    pub kz: BTreeMap<String, Money>,
    pub kz_de: BTreeMap<String, String>,
    pub output_tax: Money,
    pub input_tax: Money,
    pub zahllast: Money,
    pub note: &'static str,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct BankTxInput {
    pub booking_date: NaiveDate,
    #[serde(default)]
    pub value_date: Option<NaiveDate>,
    #[serde(default)]
    pub partner: Option<String>,
    #[serde(default)]
    pub iban: Option<String>,
    pub reference: String,
    pub amount: Money,
    #[serde(default = "eur")]
    pub currency: String,
}

fn eur() -> String {
    "EUR".into()
}

#[derive(Debug, Clone, Serialize)]
pub struct BankTx {
    pub id: i64,
    pub booking_date: NaiveDate,
    pub partner: Option<String>,
    pub iban: Option<String>,
    pub reference: String,
    pub amount: Money,
    pub amount_de: String,
    pub currency: String,
    pub entry_id: Option<i64>,
}

#[derive(Debug, Clone, Serialize)]
pub struct ImportSummary {
    pub imported: usize,
    pub duplicates: usize,
}

#[derive(Debug, Clone, Serialize)]
pub struct Draft {
    pub id: i64,
    pub source: String,
    pub kind: String,
    pub event_id: Option<String>,
    pub status: String,
    pub entry_id: Option<i64>,
    pub input: EntryInput,
}

const SCHEMA: &str = r#"
CREATE TABLE IF NOT EXISTS meta (key TEXT PRIMARY KEY, value TEXT NOT NULL);
CREATE TABLE IF NOT EXISTS accounts (
    number INTEGER PRIMARY KEY CHECK (number BETWEEN 0 AND 9999),
    name TEXT NOT NULL,
    kind TEXT NOT NULL CHECK (kind IN ('asset','liability','equity','revenue','expense')),
    default_tax_key TEXT,
    active INTEGER NOT NULL DEFAULT 1
);
CREATE TABLE IF NOT EXISTS entries (
    id INTEGER PRIMARY KEY,
    entry_date TEXT NOT NULL,
    kreis TEXT NOT NULL CHECK (kreis IN ('AR','ER','BK','KA','SO')),
    beleg_nr TEXT NOT NULL,
    description TEXT NOT NULL,
    created_at TEXT NOT NULL,
    reverses_id INTEGER REFERENCES entries(id),
    reversed_by_id INTEGER REFERENCES entries(id),
    UNIQUE (kreis, beleg_nr)
);
CREATE TABLE IF NOT EXISTS lines (
    id INTEGER PRIMARY KEY,
    entry_id INTEGER NOT NULL REFERENCES entries(id),
    account INTEGER NOT NULL REFERENCES accounts(number),
    amount_cents INTEGER NOT NULL CHECK (amount_cents <> 0),
    tax_key TEXT,
    is_tax INTEGER NOT NULL DEFAULT 0,
    memo TEXT
);
CREATE INDEX IF NOT EXISTS idx_lines_entry ON lines(entry_id);
CREATE INDEX IF NOT EXISTS idx_lines_account ON lines(account);
CREATE INDEX IF NOT EXISTS idx_entries_date ON entries(entry_date);
CREATE TABLE IF NOT EXISTS periods (month TEXT PRIMARY KEY, closed_at TEXT NOT NULL);
CREATE TABLE IF NOT EXISTS bank_tx (
    id INTEGER PRIMARY KEY,
    booking_date TEXT NOT NULL,
    value_date TEXT,
    partner TEXT,
    iban TEXT,
    reference TEXT NOT NULL,
    amount_cents INTEGER NOT NULL,
    currency TEXT NOT NULL,
    import_hash TEXT NOT NULL UNIQUE,
    entry_id INTEGER REFERENCES entries(id)
);
CREATE INDEX IF NOT EXISTS idx_bank_unmatched ON bank_tx(entry_id) WHERE entry_id IS NULL;
CREATE TABLE IF NOT EXISTS drafts (
    id INTEGER PRIMARY KEY,
    source TEXT NOT NULL,
    kind TEXT NOT NULL,
    event_id TEXT UNIQUE,
    input_json TEXT NOT NULL,
    created_at TEXT NOT NULL,
    status TEXT NOT NULL DEFAULT 'open' CHECK (status IN ('open','booked','discarded')),
    entry_id INTEGER REFERENCES entries(id)
);
CREATE TRIGGER IF NOT EXISTS lines_immutable_u BEFORE UPDATE ON lines
BEGIN SELECT RAISE(ABORT, 'booked lines are immutable; use entry_reverse'); END;
CREATE TRIGGER IF NOT EXISTS lines_immutable_d BEFORE DELETE ON lines
BEGIN SELECT RAISE(ABORT, 'booked lines are immutable; use entry_reverse'); END;
CREATE TRIGGER IF NOT EXISTS entries_immutable_d BEFORE DELETE ON entries
BEGIN SELECT RAISE(ABORT, 'booked entries are immutable; use entry_reverse'); END;
CREATE TRIGGER IF NOT EXISTS entries_immutable_u BEFORE UPDATE ON entries
WHEN NEW.entry_date IS NOT OLD.entry_date OR NEW.kreis IS NOT OLD.kreis
  OR NEW.beleg_nr IS NOT OLD.beleg_nr OR NEW.description IS NOT OLD.description
  OR NEW.reverses_id IS NOT OLD.reverses_id OR OLD.reversed_by_id IS NOT NULL
BEGIN SELECT RAISE(ABORT, 'booked entries are immutable; only a single Storno link may be added'); END;
"#;

fn d2s(d: NaiveDate) -> String {
    d.format("%Y-%m-%d").to_string()
}

fn s2d(s: &str) -> Result<NaiveDate> {
    NaiveDate::parse_from_str(s, "%Y-%m-%d")
        .map_err(|e| BuchError::Db(format!("bad stored date {s:?}: {e}")))
}

fn month_of(d: NaiveDate) -> String {
    d.format("%Y-%m").to_string()
}

/// Validate "YYYY-MM".
pub fn parse_month(s: &str) -> Result<String> {
    let t = s.trim();
    if t.len() != 7 {
        return Err(BuchError::Invalid(format!("month must be YYYY-MM: {s:?}")));
    }
    NaiveDate::parse_from_str(&format!("{t}-01"), "%Y-%m-%d")
        .map(|_| t.to_string())
        .map_err(|_| BuchError::Invalid(format!("month must be YYYY-MM: {s:?}")))
}

fn check_text(label: &str, s: &str) -> Result<()> {
    if s.trim().is_empty() {
        return Err(BuchError::Invalid(format!("{label} is empty")));
    }
    if s.chars().count() > MAX_TEXT {
        return Err(BuchError::Invalid(format!(
            "{label} longer than {MAX_TEXT} characters"
        )));
    }
    Ok(())
}

fn i2u(v: i64) -> Result<u32> {
    u32::try_from(v).map_err(|_| BuchError::Db(format!("bad account number {v}")))
}

/// A line after tax expansion, ready to insert.
#[derive(Debug, Clone, PartialEq)]
struct Expanded {
    account: u32,
    amount: Money,
    tax_key: Option<TaxKey>,
    is_tax: bool,
    memo: Option<String>,
}

/// Generate tax lines for net lines that carry a tax key.
fn expand_tax(lines: &[LineInput]) -> Result<Vec<Expanded>> {
    let mut out = Vec::with_capacity(lines.len() * 2);
    for l in lines {
        out.push(Expanded {
            account: l.account,
            amount: l.amount,
            tax_key: l.tax_key,
            is_tax: false,
            memo: l.memo.clone(),
        });
        let Some(key) = l.tax_key else { continue };
        let tax = Money::tax_from_net(l.amount, key.rate_pct())?;
        if tax.is_zero() {
            continue;
        }
        let (out_acc, in_acc) = key.tax_accounts();
        match key.direction() {
            TaxDirection::Output | TaxDirection::Input => {
                let acc = out_acc
                    .or(in_acc)
                    .ok_or_else(|| BuchError::Invalid("tax key without account".into()))?;
                out.push(Expanded {
                    account: acc,
                    amount: tax,
                    tax_key: Some(key),
                    is_tax: true,
                    memo: None,
                });
            }
            TaxDirection::ReverseCharge => {
                let (Some(o), Some(i)) = (out_acc, in_acc) else {
                    return Err(BuchError::Invalid(
                        "reverse-charge key needs two accounts".into(),
                    ));
                };
                out.push(Expanded {
                    account: i,
                    amount: tax,
                    tax_key: Some(key),
                    is_tax: true,
                    memo: None,
                });
                out.push(Expanded {
                    account: o,
                    amount: tax.checked_neg()?,
                    tax_key: Some(key),
                    is_tax: true,
                    memo: None,
                });
            }
        }
    }
    Ok(out)
}

fn debit_credit(lines: &[Expanded]) -> Result<(Money, Money)> {
    let mut d = Money::ZERO;
    let mut c = Money::ZERO;
    for l in lines {
        if l.amount.is_negative() {
            c = c.checked_add(l.amount.checked_abs()?)?;
        } else {
            d = d.checked_add(l.amount)?;
        }
    }
    Ok((d, c))
}

pub struct Ledger {
    conn: Connection,
}

impl Ledger {
    pub fn open(path: &Path) -> Result<Ledger> {
        if let Some(dir) = path.parent() {
            if !dir.as_os_str().is_empty() {
                std::fs::create_dir_all(dir).map_err(|e| BuchError::Db(e.to_string()))?;
            }
        }
        let conn = Connection::open(path)?;
        conn.execute_batch("PRAGMA journal_mode = WAL; PRAGMA synchronous = NORMAL;")?;
        Self::init(conn)
    }

    pub fn open_in_memory() -> Result<Ledger> {
        Self::init(Connection::open_in_memory()?)
    }

    fn init(conn: Connection) -> Result<Ledger> {
        conn.execute_batch("PRAGMA foreign_keys = ON; PRAGMA busy_timeout = 5000;")?;
        conn.execute_batch(SCHEMA)?;
        let version: Option<String> = conn
            .query_row(
                "SELECT value FROM meta WHERE key = 'schema_version'",
                [],
                |r| r.get(0),
            )
            .optional()?;
        match version.as_deref() {
            None => {
                conn.execute(
                    "INSERT INTO meta (key, value) VALUES ('schema_version', ?1)",
                    params![SCHEMA_VERSION.to_string()],
                )?;
            }
            Some(v) if v == SCHEMA_VERSION.to_string() => {}
            Some(v) => {
                return Err(BuchError::Db(format!(
                    "database schema {v} is newer than this build ({SCHEMA_VERSION})"
                )))
            }
        }
        Ok(Ledger { conn })
    }

    pub fn meta(&self, key: &str) -> Result<Option<String>> {
        Ok(self
            .conn
            .query_row("SELECT value FROM meta WHERE key = ?1", params![key], |r| {
                r.get(0)
            })
            .optional()?)
    }

    pub fn set_meta(&self, key: &str, value: &str) -> Result<()> {
        self.conn.execute(
            "INSERT INTO meta (key, value) VALUES (?1, ?2) ON CONFLICT(key) DO UPDATE SET value = excluded.value",
            params![key, value],
        )?;
        Ok(())
    }

    pub fn is_demo(&self) -> Result<bool> {
        Ok(self.meta("demo")?.as_deref() == Some("1"))
    }

    /// Bank clearing account for bank bookings (default 2800).
    pub fn bank_account(&self) -> Result<u32> {
        match self.meta("bank_account")? {
            Some(v) => v
                .parse()
                .map_err(|_| BuchError::Db(format!("bad bank_account meta {v:?}"))),
            None => Ok(2800),
        }
    }

    // ---------- accounts ----------

    /// Insert the EKR seed chart; existing numbers are left untouched.
    pub fn seed_chart(&mut self) -> Result<usize> {
        let tx = self.conn.transaction()?;
        let mut n = 0;
        for a in seed_accounts() {
            n += tx.execute(
                "INSERT OR IGNORE INTO accounts (number, name, kind, default_tax_key) VALUES (?1, ?2, ?3, ?4)",
                params![a.number, a.name, a.kind.as_str(), a.default_tax_key.map(TaxKey::as_str)],
            )?;
        }
        tx.commit()?;
        Ok(n)
    }

    pub fn add_account(
        &mut self,
        number: u32,
        name: &str,
        kind: AccountKind,
        default_tax_key: Option<TaxKey>,
    ) -> Result<Account> {
        craft_biz::ekr::class_of(number)?;
        check_text("account name", name)?;
        let n = self.conn.execute(
            "INSERT OR IGNORE INTO accounts (number, name, kind, default_tax_key) VALUES (?1, ?2, ?3, ?4)",
            params![number, name.trim(), kind.as_str(), default_tax_key.map(TaxKey::as_str)],
        )?;
        if n == 0 {
            return Err(BuchError::Invalid(format!(
                "account {number} already exists"
            )));
        }
        self.account(number)
    }

    pub fn account(&self, number: u32) -> Result<Account> {
        self.accounts()?
            .into_iter()
            .find(|a| a.number == number)
            .ok_or_else(|| BuchError::NotFound(format!("account {number}")))
    }

    pub fn accounts(&self) -> Result<Vec<Account>> {
        let mut stmt = self.conn.prepare(
            "SELECT number, name, kind, default_tax_key, active FROM accounts ORDER BY number",
        )?;
        let rows = stmt.query_map([], |r| {
            Ok((
                r.get::<_, i64>(0)?,
                r.get::<_, String>(1)?,
                r.get::<_, String>(2)?,
                r.get::<_, Option<String>>(3)?,
                r.get::<_, i64>(4)?,
            ))
        })?;
        let mut out = Vec::new();
        for row in rows {
            let (num, name, kind, key, active) = row?;
            out.push(Account {
                number: i2u(num)?,
                name,
                kind: AccountKind::parse(&kind)?,
                default_tax_key: key.as_deref().map(TaxKey::parse).transpose()?,
                active: active != 0,
            });
        }
        Ok(out)
    }

    // ---------- periods ----------

    fn ensure_open(conn: &Connection, date: NaiveDate) -> Result<()> {
        let m = month_of(date);
        let closed: Option<String> = conn
            .query_row(
                "SELECT month FROM periods WHERE month = ?1",
                params![m],
                |r| r.get(0),
            )
            .optional()?;
        match closed {
            Some(m) => Err(BuchError::PeriodClosed(m)),
            None => Ok(()),
        }
    }

    pub fn close_period(&mut self, month: &str) -> Result<()> {
        let m = parse_month(month)?;
        let n = self.conn.execute(
            "INSERT OR IGNORE INTO periods (month, closed_at) VALUES (?1, ?2)",
            params![m, Utc::now().to_rfc3339()],
        )?;
        if n == 0 {
            return Err(BuchError::Invalid(format!("period {m} already closed")));
        }
        Ok(())
    }

    pub fn closed_periods(&self) -> Result<Vec<String>> {
        let mut stmt = self
            .conn
            .prepare("SELECT month FROM periods ORDER BY month")?;
        let rows = stmt.query_map([], |r| r.get::<_, String>(0))?;
        Ok(rows.collect::<std::result::Result<Vec<_>, _>>()?)
    }

    // ---------- booking ----------

    fn next_beleg(tx: &Transaction<'_>, kreis: Kreis, date: NaiveDate) -> Result<String> {
        let year = date.year();
        let count: i64 = tx.query_row(
            "SELECT COUNT(*) FROM entries WHERE kreis = ?1 AND substr(entry_date, 1, 4) = ?2",
            params![kreis.as_str(), year.to_string()],
            |r| r.get(0),
        )?;
        for n in (count + 1)..(count + 10_000) {
            let cand = format!("{}-{year}-{n:04}", kreis.as_str());
            let taken: Option<i64> = tx
                .query_row(
                    "SELECT id FROM entries WHERE kreis = ?1 AND beleg_nr = ?2",
                    params![kreis.as_str(), cand],
                    |r| r.get(0),
                )
                .optional()?;
            if taken.is_none() {
                return Ok(cand);
            }
        }
        Err(BuchError::Invalid(
            "could not allocate a Belegnummer".into(),
        ))
    }

    fn book_in(
        tx: &Transaction<'_>,
        input: &EntryInput,
        reverses: Option<i64>,
        prebuilt: Option<Vec<Expanded>>,
    ) -> Result<i64> {
        check_text("description", &input.description)?;
        if input.lines.len() < 2 && prebuilt.is_none() {
            return Err(BuchError::Invalid(
                "an entry needs at least two lines".into(),
            ));
        }
        if input.lines.len() > MAX_LINES {
            return Err(BuchError::Invalid(format!("more than {MAX_LINES} lines")));
        }
        Self::ensure_open(tx, input.date)?;
        let lines = match prebuilt {
            Some(l) => l,
            None => {
                for l in &input.lines {
                    if l.amount.is_zero() {
                        return Err(BuchError::Invalid(format!(
                            "zero amount on account {}",
                            l.account
                        )));
                    }
                    if let Some(m) = &l.memo {
                        check_text("memo", m)?;
                    }
                }
                expand_tax(&input.lines)?
            }
        };
        let (debit, credit) = debit_credit(&lines)?;
        if debit != credit {
            return Err(BuchError::Unbalanced {
                debit: debit.format_de(),
                credit: credit.format_de(),
            });
        }
        for l in &lines {
            let active: Option<i64> = tx
                .query_row(
                    "SELECT active FROM accounts WHERE number = ?1",
                    params![l.account],
                    |r| r.get(0),
                )
                .optional()?;
            match active {
                None => {
                    return Err(BuchError::NotFound(format!(
                        "account {} (seed the chart or add it first)",
                        l.account
                    )))
                }
                Some(0) => {
                    return Err(BuchError::Invalid(format!(
                        "account {} is inactive",
                        l.account
                    )))
                }
                Some(_) => {}
            }
        }
        let beleg = match &input.beleg_nr {
            Some(b) => {
                check_text("beleg_nr", b)?;
                if b.chars().count() > 64 {
                    return Err(BuchError::Invalid(
                        "beleg_nr longer than 64 characters".into(),
                    ));
                }
                b.trim().to_string()
            }
            None => Self::next_beleg(tx, input.kreis, input.date)?,
        };
        tx.execute(
            "INSERT INTO entries (entry_date, kreis, beleg_nr, description, created_at, reverses_id) VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
            params![d2s(input.date), input.kreis.as_str(), beleg, input.description.trim(), Utc::now().to_rfc3339(), reverses],
        )
        .map_err(|e| match e {
            rusqlite::Error::SqliteFailure(f, _) if f.code == rusqlite::ErrorCode::ConstraintViolation => {
                BuchError::Invalid(format!("Belegnummer {beleg} already used in {}", input.kreis.as_str()))
            }
            other => BuchError::from(other),
        })?;
        let id = tx.last_insert_rowid();
        for l in &lines {
            tx.execute(
                "INSERT INTO lines (entry_id, account, amount_cents, tax_key, is_tax, memo) VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
                params![id, l.account, l.amount.cents(), l.tax_key.map(TaxKey::as_str), i64::from(l.is_tax), l.memo],
            )?;
        }
        // Second wall: re-check the stored sum.
        let sum: i64 = tx.query_row(
            "SELECT COALESCE(SUM(amount_cents), 0) FROM lines WHERE entry_id = ?1",
            params![id],
            |r| r.get(0),
        )?;
        if sum != 0 {
            return Err(BuchError::Unbalanced {
                debit: debit.format_de(),
                credit: credit.format_de(),
            });
        }
        Ok(id)
    }

    pub fn book(&mut self, input: &EntryInput) -> Result<Entry> {
        let tx = self.conn.transaction()?;
        let id = Self::book_in(&tx, input, None, None)?;
        tx.commit()?;
        self.entry(id)
    }

    /// Book a gross amount: `account` gets the net (same sign as `gross`), the
    /// engine adds the tax line, `contra` gets `-gross`.
    #[allow(clippy::too_many_arguments)]
    pub fn gross_input(
        date: NaiveDate,
        kreis: Kreis,
        description: &str,
        account: u32,
        contra: u32,
        gross: Money,
        tax_key: Option<TaxKey>,
        beleg_nr: Option<String>,
    ) -> Result<EntryInput> {
        if gross.is_zero() {
            return Err(BuchError::Invalid("gross amount is zero".into()));
        }
        let net = match tax_key {
            Some(k) if k.direction() != TaxDirection::ReverseCharge => {
                Money::split_gross(gross, k.rate_pct())?.0
            }
            _ => gross,
        };
        Ok(EntryInput {
            date,
            kreis,
            beleg_nr,
            description: description.to_string(),
            lines: vec![
                LineInput {
                    account,
                    amount: net,
                    tax_key,
                    memo: None,
                },
                LineInput {
                    account: contra,
                    amount: gross.checked_neg()?,
                    tax_key: None,
                    memo: None,
                },
            ],
        })
    }

    /// Storno: a new entry with every line negated, linked both ways.
    pub fn reverse(&mut self, id: i64, date: NaiveDate, reason: &str) -> Result<Entry> {
        check_text("reason", reason)?;
        let orig = self.entry(id)?;
        if orig.reversed_by_id.is_some() {
            return Err(BuchError::Invalid(format!(
                "entry {id} is already reversed"
            )));
        }
        if orig.reverses_id.is_some() {
            return Err(BuchError::Invalid(format!(
                "entry {id} is itself a Storno; book a new entry instead"
            )));
        }
        if date < orig.date {
            return Err(BuchError::Invalid(
                "Storno date cannot be before the original entry".into(),
            ));
        }
        let mut lines = Vec::with_capacity(orig.lines.len());
        for l in &orig.lines {
            lines.push(Expanded {
                account: l.account,
                amount: l.amount.checked_neg()?,
                tax_key: l.tax_key.as_deref().map(TaxKey::parse).transpose()?,
                is_tax: l.is_tax,
                memo: l.memo.clone(),
            });
        }
        let input = EntryInput {
            date,
            kreis: orig.kreis,
            beleg_nr: Some(format!("ST-{}", orig.beleg_nr).chars().take(64).collect()),
            description: format!("Storno {}: {}", orig.beleg_nr, reason.trim())
                .chars()
                .take(MAX_TEXT)
                .collect(),
            lines: Vec::new(),
        };
        let tx = self.conn.transaction()?;
        let new_id = Self::book_in(&tx, &input, Some(id), Some(lines))?;
        tx.execute(
            "UPDATE entries SET reversed_by_id = ?1 WHERE id = ?2",
            params![new_id, id],
        )?;
        tx.commit()?;
        self.entry(new_id)
    }

    // ---------- reads ----------

    fn lines_of(&self, entry_id: i64) -> Result<Vec<Line>> {
        let mut stmt = self.conn.prepare(
            "SELECT l.id, l.account, a.name, l.amount_cents, l.tax_key, l.is_tax, l.memo
             FROM lines l JOIN accounts a ON a.number = l.account WHERE l.entry_id = ?1 ORDER BY l.id",
        )?;
        let rows = stmt.query_map(params![entry_id], |r| {
            Ok((
                r.get::<_, i64>(0)?,
                r.get::<_, i64>(1)?,
                r.get::<_, String>(2)?,
                r.get::<_, i64>(3)?,
                r.get::<_, Option<String>>(4)?,
                r.get::<_, i64>(5)?,
                r.get::<_, Option<String>>(6)?,
            ))
        })?;
        let mut out = Vec::new();
        for row in rows {
            let (id, acc, name, cents, key, is_tax, memo) = row?;
            let amount = Money::from_cents(cents);
            out.push(Line {
                id,
                account: i2u(acc)?,
                account_name: name,
                amount,
                amount_de: amount.format_de(),
                tax_key: key,
                is_tax: is_tax != 0,
                memo,
            });
        }
        Ok(out)
    }

    pub fn entry(&self, id: i64) -> Result<Entry> {
        let row = self
            .conn
            .query_row(
                "SELECT id, entry_date, kreis, beleg_nr, description, created_at, reverses_id, reversed_by_id FROM entries WHERE id = ?1",
                params![id],
                |r| Ok((r.get::<_, i64>(0)?, r.get::<_, String>(1)?, r.get::<_, String>(2)?, r.get::<_, String>(3)?, r.get::<_, String>(4)?, r.get::<_, String>(5)?, r.get::<_, Option<i64>>(6)?, r.get::<_, Option<i64>>(7)?)),
            )
            .optional()?
            .ok_or_else(|| BuchError::NotFound(format!("entry {id}")))?;
        let (id, date, kreis, beleg_nr, description, created_at, reverses_id, reversed_by_id) = row;
        Ok(Entry {
            id,
            date: s2d(&date)?,
            kreis: Kreis::parse(&kreis)?,
            beleg_nr,
            description,
            created_at,
            reverses_id,
            reversed_by_id,
            lines: self.lines_of(id)?,
        })
    }

    pub fn journal(
        &self,
        from: NaiveDate,
        to: NaiveDate,
        kreis: Option<Kreis>,
        limit: usize,
    ) -> Result<Vec<Entry>> {
        let mut stmt = self.conn.prepare(
            "SELECT id FROM entries WHERE entry_date BETWEEN ?1 AND ?2 AND (?3 IS NULL OR kreis = ?3)
             ORDER BY entry_date, id LIMIT ?4",
        )?;
        let lim = i64::try_from(limit.min(10_000)).unwrap_or(10_000);
        let ids = stmt.query_map(
            params![d2s(from), d2s(to), kreis.map(Kreis::as_str), lim],
            |r| r.get::<_, i64>(0),
        )?;
        let mut out = Vec::new();
        for id in ids {
            out.push(self.entry(id?)?);
        }
        Ok(out)
    }

    /// Saldenliste for a date range (inclusive).
    pub fn balances(&self, from: NaiveDate, to: NaiveDate) -> Result<Balances> {
        let mut stmt = self.conn.prepare(
            "SELECT a.number, a.name, a.kind,
                    COALESCE(SUM(CASE WHEN l.amount_cents > 0 THEN l.amount_cents END), 0),
                    COALESCE(SUM(CASE WHEN l.amount_cents < 0 THEN -l.amount_cents END), 0)
             FROM lines l JOIN entries e ON e.id = l.entry_id JOIN accounts a ON a.number = l.account
             WHERE e.entry_date BETWEEN ?1 AND ?2
             GROUP BY a.number ORDER BY a.number",
        )?;
        let rows = stmt.query_map(params![d2s(from), d2s(to)], |r| {
            Ok((
                r.get::<_, i64>(0)?,
                r.get::<_, String>(1)?,
                r.get::<_, String>(2)?,
                r.get::<_, i64>(3)?,
                r.get::<_, i64>(4)?,
            ))
        })?;
        let mut out = Vec::new();
        let mut td = Money::ZERO;
        let mut tc = Money::ZERO;
        for row in rows {
            let (num, name, kind, d, c) = row?;
            let debit = Money::from_cents(d);
            let credit = Money::from_cents(c);
            let saldo = debit.checked_sub(credit)?;
            td = td.checked_add(debit)?;
            tc = tc.checked_add(credit)?;
            out.push(BalanceRow {
                account: i2u(num)?,
                name,
                kind: AccountKind::parse(&kind)?,
                debit,
                credit,
                saldo,
                saldo_de: saldo.format_de(),
            });
        }
        Ok(Balances {
            from,
            to,
            rows: out,
            total_debit: td,
            total_credit: tc,
            total_debit_de: td.format_de(),
            total_credit_de: tc.format_de(),
        })
    }

    /// Kontoblatt with running balance. Opening balance is everything before `from`.
    pub fn account_ledger(
        &self,
        account: u32,
        from: NaiveDate,
        to: NaiveDate,
    ) -> Result<(Money, Vec<LedgerRow>)> {
        self.account(account)?;
        let opening: i64 = self.conn.query_row(
            "SELECT COALESCE(SUM(l.amount_cents), 0) FROM lines l JOIN entries e ON e.id = l.entry_id WHERE l.account = ?1 AND e.entry_date < ?2",
            params![account, d2s(from)],
            |r| r.get(0),
        )?;
        let opening = Money::from_cents(opening);
        let mut stmt = self.conn.prepare(
            "SELECT e.id, e.entry_date, e.beleg_nr, e.description, l.amount_cents
             FROM lines l JOIN entries e ON e.id = l.entry_id
             WHERE l.account = ?1 AND e.entry_date BETWEEN ?2 AND ?3 ORDER BY e.entry_date, e.id, l.id",
        )?;
        let rows = stmt.query_map(params![account, d2s(from), d2s(to)], |r| {
            Ok((
                r.get::<_, i64>(0)?,
                r.get::<_, String>(1)?,
                r.get::<_, String>(2)?,
                r.get::<_, String>(3)?,
                r.get::<_, i64>(4)?,
            ))
        })?;
        let mut running = opening;
        let mut out = Vec::new();
        for row in rows {
            let (id, date, beleg, desc, cents) = row?;
            let amount = Money::from_cents(cents);
            running = running.checked_add(amount)?;
            out.push(LedgerRow {
                entry_id: id,
                date: s2d(&date)?,
                beleg_nr: beleg,
                description: desc,
                amount,
                running,
                running_de: running.format_de(),
            });
        }
        Ok((opening, out))
    }

    /// U30 pre-fill for one month. Not a filing.
    pub fn u30(&self, month: &str) -> Result<U30Report> {
        let m = parse_month(month)?;
        let mut stmt = self.conn.prepare(
            "SELECT l.account, l.amount_cents, l.tax_key, l.is_tax FROM lines l JOIN entries e ON e.id = l.entry_id
             WHERE substr(e.entry_date, 1, 7) = ?1 AND l.tax_key IS NOT NULL",
        )?;
        let rows = stmt.query_map(params![m], |r| {
            Ok((
                r.get::<_, i64>(0)?,
                r.get::<_, i64>(1)?,
                r.get::<_, String>(2)?,
                r.get::<_, i64>(3)?,
            ))
        })?;
        let mut kz: BTreeMap<String, Money> = BTreeMap::new();
        let mut add = |k: &str, v: Money| -> Result<()> {
            let cur = kz.get(k).copied().unwrap_or(Money::ZERO);
            kz.insert(k.to_string(), cur.checked_add(v)?);
            Ok(())
        };
        let mut output_tax = Money::ZERO;
        for row in rows {
            let (acc, cents, key, is_tax) = row?;
            let key = TaxKey::parse(&key)?;
            let amount = Money::from_cents(cents);
            let f = key.u30();
            if is_tax == 0 {
                let base = match key.direction() {
                    TaxDirection::Output => amount.checked_neg()?,
                    TaxDirection::Input => continue,
                    TaxDirection::ReverseCharge => amount,
                };
                if let Some(b) = f.base_kz {
                    add(b, base)?;
                }
                if key.direction() == TaxDirection::ReverseCharge {
                    add("070", base)?;
                }
                if f.in_kz_000 {
                    add("000", base)?;
                }
            } else {
                let (out_acc, in_acc) = key.tax_accounts();
                let acc = i2u(acc)?;
                if Some(acc) == out_acc {
                    output_tax = output_tax.checked_add(amount.checked_neg()?)?;
                } else if Some(acc) == in_acc {
                    if let Some(t) = f.tax_kz {
                        add(t, amount)?;
                    }
                }
            }
        }
        let input_tax = Money::sum(
            ["060", "065"]
                .iter()
                .map(|k| kz.get(*k).copied().unwrap_or(Money::ZERO)),
        )?;
        let zahllast = output_tax.checked_sub(input_tax)?;
        kz.insert("095".into(), zahllast);
        let kz_de = kz.iter().map(|(k, v)| (k.clone(), v.format_de())).collect();
        Ok(U30Report {
            month: m,
            kz,
            kz_de,
            output_tax,
            input_tax,
            zahllast,
            note: "Pre-fill from booked entries. Not a filing; check against FinanzOnline before submitting. Positive 095 = payable.",
        })
    }

    // ---------- bank ----------

    pub fn import_bank(&mut self, txs: &[BankTxInput]) -> Result<ImportSummary> {
        let tx = self.conn.transaction()?;
        let mut seen: HashMap<String, u32> = HashMap::new();
        let mut imported = 0;
        let mut duplicates = 0;
        for t in txs {
            check_text("reference", &t.reference)?;
            let base = format!(
                "{}|{}|{}|{}",
                d2s(t.booking_date),
                t.amount.cents(),
                t.iban.as_deref().unwrap_or(""),
                t.reference.trim()
            );
            let n = seen.entry(base.clone()).or_insert(0);
            *n += 1;
            let hash = format!("{:x}", Sha256::digest(format!("{base}|{n}").as_bytes()));
            let inserted = tx.execute(
                "INSERT OR IGNORE INTO bank_tx (booking_date, value_date, partner, iban, reference, amount_cents, currency, import_hash)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)",
                params![d2s(t.booking_date), t.value_date.map(d2s), t.partner, t.iban, t.reference.trim(), t.amount.cents(), t.currency, hash],
            )?;
            if inserted == 0 {
                duplicates += 1;
            } else {
                imported += 1;
            }
        }
        tx.commit()?;
        Ok(ImportSummary {
            imported,
            duplicates,
        })
    }

    fn bank_rows(&self, sql: &str, p: &[&dyn rusqlite::ToSql]) -> Result<Vec<BankTx>> {
        let mut stmt = self.conn.prepare(sql)?;
        let rows = stmt.query_map(p, |r| {
            Ok((
                r.get::<_, i64>(0)?,
                r.get::<_, String>(1)?,
                r.get::<_, Option<String>>(2)?,
                r.get::<_, Option<String>>(3)?,
                r.get::<_, String>(4)?,
                r.get::<_, i64>(5)?,
                r.get::<_, String>(6)?,
                r.get::<_, Option<i64>>(7)?,
            ))
        })?;
        let mut out = Vec::new();
        for row in rows {
            let (id, date, partner, iban, reference, cents, currency, entry_id) = row?;
            let amount = Money::from_cents(cents);
            out.push(BankTx {
                id,
                booking_date: s2d(&date)?,
                partner,
                iban,
                reference,
                amount,
                amount_de: amount.format_de(),
                currency,
                entry_id,
            });
        }
        Ok(out)
    }

    pub fn bank_unmatched(&self, limit: usize) -> Result<Vec<BankTx>> {
        let lim = i64::try_from(limit.min(10_000)).unwrap_or(10_000);
        self.bank_rows(
            "SELECT id, booking_date, partner, iban, reference, amount_cents, currency, entry_id FROM bank_tx
             WHERE entry_id IS NULL ORDER BY booking_date, id LIMIT ?1",
            &[&lim as &dyn rusqlite::ToSql],
        )
    }

    /// Book an unmatched bank line against `account` and link it.
    pub fn bank_book(
        &mut self,
        bank_tx_id: i64,
        account: u32,
        tax_key: Option<TaxKey>,
        description: Option<&str>,
    ) -> Result<Entry> {
        let t = self
            .bank_rows(
                "SELECT id, booking_date, partner, iban, reference, amount_cents, currency, entry_id FROM bank_tx WHERE id = ?1",
                &[&bank_tx_id as &dyn rusqlite::ToSql],
            )?
            .into_iter()
            .next()
            .ok_or_else(|| BuchError::NotFound(format!("bank transaction {bank_tx_id}")))?;
        if t.entry_id.is_some() {
            return Err(BuchError::Invalid(format!(
                "bank transaction {bank_tx_id} is already booked"
            )));
        }
        if t.currency != "EUR" {
            return Err(BuchError::Invalid(format!(
                "only EUR is supported, got {}",
                t.currency
            )));
        }
        let bank = self.bank_account()?;
        let desc = match description {
            Some(d) => d.to_string(),
            None => format!("{} {}", t.partner.clone().unwrap_or_default(), t.reference)
                .trim()
                .chars()
                .take(MAX_TEXT)
                .collect(),
        };
        // Money into the bank (positive) is a credit on the counter account.
        let input = Self::gross_input(
            t.booking_date,
            Kreis::BK,
            &desc,
            account,
            bank,
            t.amount.checked_neg()?,
            tax_key,
            None,
        )?;
        let tx = self.conn.transaction()?;
        let id = Self::book_in(&tx, &input, None, None)?;
        tx.execute(
            "UPDATE bank_tx SET entry_id = ?1 WHERE id = ?2 AND entry_id IS NULL",
            params![id, bank_tx_id],
        )?;
        tx.commit()?;
        self.entry(id)
    }

    // ---------- drafts (from other apps via the bus) ----------

    pub fn add_draft(
        &mut self,
        source: &str,
        kind: &str,
        event_id: Option<&str>,
        input: &EntryInput,
    ) -> Result<Option<i64>> {
        let json = serde_json::to_string(input)?;
        let n = self.conn.execute(
            "INSERT OR IGNORE INTO drafts (source, kind, event_id, input_json, created_at) VALUES (?1, ?2, ?3, ?4, ?5)",
            params![source, kind, event_id, json, Utc::now().to_rfc3339()],
        )?;
        Ok(if n == 0 {
            None
        } else {
            Some(self.conn.last_insert_rowid())
        })
    }

    pub fn drafts(&self, only_open: bool) -> Result<Vec<Draft>> {
        let mut stmt = self.conn.prepare(
            "SELECT id, source, kind, event_id, status, entry_id, input_json FROM drafts WHERE (?1 = 0 OR status = 'open') ORDER BY id",
        )?;
        let rows = stmt.query_map(params![i64::from(only_open)], |r| {
            Ok((
                r.get::<_, i64>(0)?,
                r.get::<_, String>(1)?,
                r.get::<_, String>(2)?,
                r.get::<_, Option<String>>(3)?,
                r.get::<_, String>(4)?,
                r.get::<_, Option<i64>>(5)?,
                r.get::<_, String>(6)?,
            ))
        })?;
        let mut out = Vec::new();
        for row in rows {
            let (id, source, kind, event_id, status, entry_id, json) = row?;
            out.push(Draft {
                id,
                source,
                kind,
                event_id,
                status,
                entry_id,
                input: serde_json::from_str(&json)?,
            });
        }
        Ok(out)
    }

    pub fn approve_draft(&mut self, id: i64) -> Result<Entry> {
        let d = self
            .drafts(false)?
            .into_iter()
            .find(|d| d.id == id)
            .ok_or_else(|| BuchError::NotFound(format!("draft {id}")))?;
        if d.status != "open" {
            return Err(BuchError::Invalid(format!("draft {id} is {}", d.status)));
        }
        let tx = self.conn.transaction()?;
        let entry_id = Self::book_in(&tx, &d.input, None, None)?;
        tx.execute(
            "UPDATE drafts SET status = 'booked', entry_id = ?1 WHERE id = ?2",
            params![entry_id, id],
        )?;
        tx.commit()?;
        self.entry(entry_id)
    }

    pub fn discard_draft(&mut self, id: i64) -> Result<()> {
        let n = self.conn.execute(
            "UPDATE drafts SET status = 'discarded' WHERE id = ?1 AND status = 'open'",
            params![id],
        )?;
        if n == 0 {
            return Err(BuchError::NotFound(format!("open draft {id}")));
        }
        Ok(())
    }

    /// Earliest and latest entry date, if any entries exist.
    pub fn entry_date_span(&self) -> Result<Option<(NaiveDate, NaiveDate)>> {
        let row: (Option<String>, Option<String>) = self.conn.query_row(
            "SELECT MIN(entry_date), MAX(entry_date) FROM entries",
            [],
            |r| Ok((r.get(0)?, r.get(1)?)),
        )?;
        match row {
            (Some(a), Some(b)) => Ok(Some((s2d(&a)?, s2d(&b)?))),
            _ => Ok(None),
        }
    }

    /// Counts for the status tool.
    pub fn stats(&self) -> Result<serde_json::Value> {
        let q = |sql: &str| -> Result<i64> { Ok(self.conn.query_row(sql, [], |r| r.get(0))?) };
        Ok(serde_json::json!({
            "accounts": q("SELECT COUNT(*) FROM accounts")?,
            "entries": q("SELECT COUNT(*) FROM entries")?,
            "lines": q("SELECT COUNT(*) FROM lines")?,
            "bank_unmatched": q("SELECT COUNT(*) FROM bank_tx WHERE entry_id IS NULL")?,
            "drafts_open": q("SELECT COUNT(*) FROM drafts WHERE status = 'open'")?,
            "closed_periods": self.closed_periods()?,
            "demo": self.is_demo()?,
        }))
    }
}

#[cfg(test)]
mod tests;
