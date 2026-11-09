//! buchcraft-core: the command bus.
//!
//! Every capability is a `Command`. The CLI, the MCP server and the desktop
//! app all build a `Command` and call `dispatch`, so the three surfaces cannot
//! disagree. `tools()` is the MCP catalog; `Command::from_tool` maps a tool
//! call onto the bus.
#![deny(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

pub mod bankcsv;
pub mod demo;

use buchcraft_store::{BuchError, EntryInput, Kreis, Ledger, LineInput};
use chrono::NaiveDate;
use craft_biz::at::parse_date;
use craft_biz::bus::Bus;
use craft_biz::ekr::{AccountKind, TaxKey};
use craft_biz::mcp::tool;
use craft_biz::Money;
use serde_json::{json, Value};
use std::path::{Path, PathBuf};

pub const APP: &str = "buchcraft";
pub const VERSION: &str = env!("CARGO_PKG_VERSION");

/// File-system roots the file-touching commands may use. `None` = unrestricted
/// (interactive CLI); `Some(empty)` = no file access (MCP without --allow-path).
#[derive(Debug, Clone, Default)]
pub struct Confinement(pub Option<Vec<PathBuf>>);

impl Confinement {
    pub fn roots(paths: &[PathBuf]) -> Result<Confinement, String> {
        let mut out = Vec::new();
        for p in paths {
            out.push(
                p.canonicalize()
                    .map_err(|e| format!("--allow-path {}: {e}", p.display()))?,
            );
        }
        Ok(Confinement(Some(out)))
    }

    /// Resolve `path` for reading (must exist) or writing (parent must exist),
    /// refusing anything outside the allowed roots.
    pub fn check(&self, path: &str, write: bool) -> Result<PathBuf, String> {
        if path.trim().is_empty() || path.len() > 1024 {
            return Err("path empty or too long".into());
        }
        let p = Path::new(path);
        let resolved = if write {
            let name = p.file_name().ok_or("path has no file name")?;
            let parent = match p.parent() {
                Some(d) if !d.as_os_str().is_empty() => d.to_path_buf(),
                _ => PathBuf::from("."),
            };
            parent
                .canonicalize()
                .map_err(|e| format!("{}: {e}", parent.display()))?
                .join(name)
        } else {
            p.canonicalize().map_err(|e| format!("{path}: {e}"))?
        };
        match &self.0 {
            None => Ok(resolved),
            Some(roots) if roots.iter().any(|r| resolved.starts_with(r)) => Ok(resolved),
            Some(roots) if roots.is_empty() => {
                Err("file access disabled: start the MCP server with --allow-path <DIR>".into())
            }
            Some(_) => Err(format!(
                "{} is outside the allowed paths",
                resolved.display()
            )),
        }
    }
}

pub struct Ctx {
    pub ledger: Ledger,
    pub confinement: Confinement,
    pub bus: Bus,
}

#[derive(Debug, Clone, PartialEq)]
pub enum Command {
    Status,
    AccountsList,
    AccountsSeed,
    AccountAdd {
        number: u32,
        name: String,
        kind: AccountKind,
        tax_key: Option<TaxKey>,
    },
    EntryBook(EntryInput),
    EntryBookGross {
        date: NaiveDate,
        kreis: Kreis,
        description: String,
        account: u32,
        contra: u32,
        gross: Money,
        tax_key: Option<TaxKey>,
        beleg_nr: Option<String>,
    },
    EntryReverse {
        id: i64,
        date: NaiveDate,
        reason: String,
    },
    EntryShow {
        id: i64,
    },
    Journal {
        from: NaiveDate,
        to: NaiveDate,
        kreis: Option<Kreis>,
        limit: usize,
    },
    Balance {
        from: NaiveDate,
        to: NaiveDate,
    },
    AccountLedger {
        account: u32,
        from: NaiveDate,
        to: NaiveDate,
    },
    U30 {
        month: String,
    },
    PeriodClose {
        month: String,
    },
    BankImport {
        path: String,
    },
    BankUnmatched {
        limit: usize,
    },
    BankBook {
        id: i64,
        account: u32,
        tax_key: Option<TaxKey>,
        description: Option<String>,
    },
    Drafts {
        all: bool,
    },
    DraftApprove {
        id: i64,
    },
    DraftDiscard {
        id: i64,
    },
    InboxDrain,
    ExportCsv {
        from: NaiveDate,
        to: NaiveDate,
        path: String,
    },
    DemoSeed,
}

// ---------- argument helpers (hostile input fence) ----------

fn s<'a>(a: &'a Value, k: &str) -> Result<&'a str, String> {
    a.get(k)
        .and_then(Value::as_str)
        .ok_or_else(|| format!("missing string argument {k:?}"))
}
fn opt_s(a: &Value, k: &str) -> Option<String> {
    a.get(k)
        .and_then(Value::as_str)
        .map(str::to_string)
        .filter(|v| !v.trim().is_empty())
}
fn int(a: &Value, k: &str) -> Result<i64, String> {
    a.get(k)
        .and_then(Value::as_i64)
        .ok_or_else(|| format!("missing integer argument {k:?}"))
}
fn acct(a: &Value, k: &str) -> Result<u32, String> {
    let v = int(a, k)?;
    u32::try_from(v)
        .ok()
        .filter(|n| *n <= 9999)
        .ok_or_else(|| format!("{k} must be a 4-digit account number"))
}
fn date(a: &Value, k: &str) -> Result<NaiveDate, String> {
    parse_date(s(a, k)?).map_err(|e| e.to_string())
}
fn tax(a: &Value, k: &str) -> Result<Option<TaxKey>, String> {
    opt_s(a, k)
        .map(|v| TaxKey::parse(&v).map_err(|e| e.to_string()))
        .transpose()
}
fn limit(a: &Value) -> usize {
    a.get("limit")
        .and_then(Value::as_u64)
        .map_or(200, |v| v.min(10_000) as usize)
}
/// An amount given either as a DE string ("-1.000,00") or as integer cents.
fn money(a: &Value, k: &str) -> Result<Money, String> {
    if let Some(c) = a.get(format!("{k}_cents")).and_then(Value::as_i64) {
        return Ok(Money::from_cents(c));
    }
    match a.get(k) {
        Some(Value::String(v)) => Money::parse_de(v).map_err(|e| e.to_string()),
        Some(Value::Number(_)) => Err(format!(
            "{k}: give a string like \"1.250,50\" or {k}_cents as an integer, not a float"
        )),
        _ => Err(format!("missing amount {k:?} (or {k}_cents)")),
    }
}

fn lines_arg(a: &Value) -> Result<Vec<LineInput>, String> {
    let arr = a
        .get("lines")
        .and_then(Value::as_array)
        .ok_or("missing array argument \"lines\"")?;
    if arr.len() > 200 {
        return Err("more than 200 lines".into());
    }
    arr.iter()
        .map(|l| -> Result<LineInput, String> {
            Ok(LineInput {
                account: acct(l, "account")?,
                amount: money(l, "amount")?,
                tax_key: tax(l, "tax_key")?,
                memo: opt_s(l, "memo"),
            })
        })
        .collect()
}

impl Command {
    /// Map an MCP tool call onto the bus. Tool names are `buch_*` (dots accepted).
    pub fn from_tool(name: &str, a: &Value) -> Result<Command, String> {
        let name = craft_biz::mcp::normalize_tool_name(name);
        Ok(match name.as_str() {
            "buch_status" => Command::Status,
            "buch_accounts_list" => Command::AccountsList,
            "buch_accounts_seed" => Command::AccountsSeed,
            "buch_account_add" => Command::AccountAdd {
                number: acct(a, "number")?,
                name: s(a, "name")?.to_string(),
                kind: AccountKind::parse(s(a, "kind")?).map_err(|e| e.to_string())?,
                tax_key: tax(a, "tax_key")?,
            },
            "buch_entry_book" => Command::EntryBook(EntryInput {
                date: date(a, "date")?,
                kreis: Kreis::parse(s(a, "kreis")?).map_err(|e| e.to_string())?,
                beleg_nr: opt_s(a, "beleg_nr"),
                description: s(a, "description")?.to_string(),
                lines: lines_arg(a)?,
            }),
            "buch_entry_book_gross" => Command::EntryBookGross {
                date: date(a, "date")?,
                kreis: Kreis::parse(s(a, "kreis")?).map_err(|e| e.to_string())?,
                description: s(a, "description")?.to_string(),
                account: acct(a, "account")?,
                contra: acct(a, "contra")?,
                gross: money(a, "gross")?,
                tax_key: tax(a, "tax_key")?,
                beleg_nr: opt_s(a, "beleg_nr"),
            },
            "buch_entry_reverse" => Command::EntryReverse {
                id: int(a, "id")?,
                date: date(a, "date")?,
                reason: s(a, "reason")?.to_string(),
            },
            "buch_entry_show" => Command::EntryShow { id: int(a, "id")? },
            "buch_journal" => Command::Journal {
                from: date(a, "from")?,
                to: date(a, "to")?,
                kreis: opt_s(a, "kreis")
                    .map(|k| Kreis::parse(&k).map_err(|e| e.to_string()))
                    .transpose()?,
                limit: limit(a),
            },
            "buch_balance" => Command::Balance {
                from: date(a, "from")?,
                to: date(a, "to")?,
            },
            "buch_account_ledger" => Command::AccountLedger {
                account: acct(a, "account")?,
                from: date(a, "from")?,
                to: date(a, "to")?,
            },
            "buch_u30" => Command::U30 {
                month: s(a, "month")?.to_string(),
            },
            "buch_period_close" => Command::PeriodClose {
                month: s(a, "month")?.to_string(),
            },
            "buch_bank_import" => Command::BankImport {
                path: s(a, "path")?.to_string(),
            },
            "buch_bank_unmatched" => Command::BankUnmatched { limit: limit(a) },
            "buch_bank_book" => Command::BankBook {
                id: int(a, "id")?,
                account: acct(a, "account")?,
                tax_key: tax(a, "tax_key")?,
                description: opt_s(a, "description"),
            },
            "buch_drafts" => Command::Drafts {
                all: a.get("all").and_then(Value::as_bool).unwrap_or(false),
            },
            "buch_draft_approve" => Command::DraftApprove { id: int(a, "id")? },
            "buch_draft_discard" => Command::DraftDiscard { id: int(a, "id")? },
            "buch_inbox_drain" => Command::InboxDrain,
            "buch_export_csv" => Command::ExportCsv {
                from: date(a, "from")?,
                to: date(a, "to")?,
                path: s(a, "path")?.to_string(),
            },
            "buch_demo_seed" => Command::DemoSeed,
            other => return Err(format!("unknown tool {other}")),
        })
    }
}

fn err(e: BuchError) -> String {
    e.to_string()
}

fn to_json<T: serde::Serialize>(v: &T) -> Result<Value, String> {
    serde_json::to_value(v).map_err(|e| e.to_string())
}

/// Execute a command. Never panics; errors are plain strings for the transport.
pub fn dispatch(ctx: &mut Ctx, cmd: Command) -> Result<Value, String> {
    let l = &mut ctx.ledger;
    match cmd {
        Command::Status => Ok(
            json!({ "app": APP, "version": VERSION, "stats": l.stats().map_err(err)?, "bus_root": ctx.bus.root().display().to_string() }),
        ),
        Command::AccountsList => Ok(json!({ "accounts": to_json(&l.accounts().map_err(err)?)? })),
        Command::AccountsSeed => Ok(json!({ "inserted": l.seed_chart().map_err(err)? })),
        Command::AccountAdd {
            number,
            name,
            kind,
            tax_key,
        } => to_json(&l.add_account(number, &name, kind, tax_key).map_err(err)?),
        Command::EntryBook(input) => to_json(&l.book(&input).map_err(err)?),
        Command::EntryBookGross {
            date,
            kreis,
            description,
            account,
            contra,
            gross,
            tax_key,
            beleg_nr,
        } => {
            let input = Ledger::gross_input(
                date,
                kreis,
                &description,
                account,
                contra,
                gross,
                tax_key,
                beleg_nr,
            )
            .map_err(err)?;
            to_json(&l.book(&input).map_err(err)?)
        }
        Command::EntryReverse { id, date, reason } => {
            to_json(&l.reverse(id, date, &reason).map_err(err)?)
        }
        Command::EntryShow { id } => to_json(&l.entry(id).map_err(err)?),
        Command::Journal {
            from,
            to,
            kreis,
            limit,
        } => Ok(json!({ "entries": to_json(&l.journal(from, to, kreis, limit).map_err(err)?)? })),
        Command::Balance { from, to } => to_json(&l.balances(from, to).map_err(err)?),
        Command::AccountLedger { account, from, to } => {
            let (opening, rows) = l.account_ledger(account, from, to).map_err(err)?;
            Ok(
                json!({ "account": account, "opening": opening, "opening_de": opening.format_de(), "rows": to_json(&rows)? }),
            )
        }
        Command::U30 { month } => to_json(&l.u30(&month).map_err(err)?),
        Command::PeriodClose { month } => {
            l.close_period(&month).map_err(err)?;
            Ok(json!({ "closed": month }))
        }
        Command::BankImport { path } => {
            let p = ctx.confinement.check(&path, false)?;
            let meta = std::fs::metadata(&p).map_err(|e| e.to_string())?;
            if meta.len() > bankcsv::MAX_FILE_BYTES as u64 {
                return Err("file too large".into());
            }
            let bytes = std::fs::read(&p).map_err(|e| e.to_string())?;
            let rows = bankcsv::parse_bank_csv(&bytes)?;
            to_json(&l.import_bank(&rows).map_err(err)?)
        }
        Command::BankUnmatched { limit } => {
            Ok(json!({ "transactions": to_json(&l.bank_unmatched(limit).map_err(err)?)? }))
        }
        Command::BankBook {
            id,
            account,
            tax_key,
            description,
        } => to_json(
            &l.bank_book(id, account, tax_key, description.as_deref())
                .map_err(err)?,
        ),
        Command::Drafts { all } => Ok(json!({ "drafts": to_json(&l.drafts(!all).map_err(err)?)? })),
        Command::DraftApprove { id } => to_json(&l.approve_draft(id).map_err(err)?),
        Command::DraftDiscard { id } => {
            l.discard_draft(id).map_err(err)?;
            Ok(json!({ "discarded": id }))
        }
        Command::InboxDrain => inbox_drain(ctx),
        Command::ExportCsv { from, to, path } => {
            let p = ctx.confinement.check(&path, true)?;
            let n = export_csv(&ctx.ledger, from, to, &p)?;
            Ok(json!({ "path": p.display().to_string(), "lines": n }))
        }
        Command::DemoSeed => demo::seed(&mut ctx.ledger),
    }
}

/// Turn bus events into drafts. Nothing is booked until a human approves.
pub fn inbox_drain(ctx: &mut Ctx) -> Result<Value, String> {
    let events = ctx.bus.pending(APP).map_err(|e| e.to_string())?;
    let mut drafted = Vec::new();
    let mut rejected = Vec::new();
    for ev in events {
        let result = match ev.kind.as_str() {
            "asset.depreciation_due" => depreciation_draft(&ev.payload).and_then(|input| {
                ctx.ledger
                    .add_draft(&ev.source, &ev.kind, Some(&ev.id), &input)
                    .map_err(err)
            }),
            other => Err(format!("buchcraft does not handle event kind {other}")),
        };
        match result {
            Ok(id) => {
                ctx.bus.ack(APP, &ev.id).map_err(|e| e.to_string())?;
                drafted.push(json!({ "event": ev.id, "draft_id": id }));
            }
            Err(e) => {
                ctx.bus.reject(APP, &ev.id, &e).map_err(|e| e.to_string())?;
                rejected.push(json!({ "event": ev.id, "error": e }));
            }
        }
    }
    Ok(json!({ "drafted": drafted, "rejected": rejected }))
}

fn depreciation_draft(p: &Value) -> Result<EntryInput, String> {
    let tag = s(p, "asset_tag")?;
    let year = i32::try_from(int(p, "year")?).map_err(|_| "bad year")?;
    let amount = Money::from_cents(int(p, "amount_cents")?);
    if amount.is_zero() || amount.is_negative() {
        return Err("amount_cents must be positive".into());
    }
    let date = match opt_s(p, "date") {
        Some(d) => parse_date(&d).map_err(|e| e.to_string())?,
        None => NaiveDate::from_ymd_opt(year, 12, 31).ok_or("bad year")?,
    };
    Ok(EntryInput {
        date,
        kreis: Kreis::SO,
        beleg_nr: None,
        description: format!("AfA {year} {tag}"),
        lines: vec![
            LineInput {
                account: acct(p, "debit")?,
                amount,
                tax_key: None,
                memo: None,
            },
            LineInput {
                account: acct(p, "credit")?,
                amount: amount.checked_neg().map_err(|e| e.to_string())?,
                tax_key: None,
                memo: None,
            },
        ],
    })
}

fn export_csv(l: &Ledger, from: NaiveDate, to: NaiveDate, path: &Path) -> Result<usize, String> {
    let entries = l.journal(from, to, None, 10_000).map_err(err)?;
    let mut w = csv::WriterBuilder::new()
        .delimiter(b';')
        .from_path(path)
        .map_err(|e| e.to_string())?;
    w.write_record([
        "Datum",
        "Kreis",
        "Beleg",
        "Konto",
        "Kontoname",
        "Betrag",
        "Steuer",
        "Steuerzeile",
        "Text",
    ])
    .map_err(|e| e.to_string())?;
    let mut n = 0;
    for e in &entries {
        for line in &e.lines {
            w.write_record([
                craft_biz::at::format_date_de(e.date),
                e.kreis.as_str().to_string(),
                e.beleg_nr.clone(),
                format!("{:04}", line.account),
                line.account_name.clone(),
                line.amount.format_de(),
                line.tax_key.clone().unwrap_or_default(),
                if line.is_tax {
                    "ja".into()
                } else {
                    "nein".into()
                },
                e.description.clone(),
            ])
            .map_err(|e| e.to_string())?;
            n += 1;
        }
    }
    w.flush().map_err(|e| e.to_string())?;
    Ok(n)
}

fn date_props(keys: &[&str]) -> serde_json::Map<String, Value> {
    keys.iter()
        .map(|k| {
            (
                k.to_string(),
                json!({ "type": "string", "description": "DD.MM.YYYY or YYYY-MM-DD" }),
            )
        })
        .collect()
}

/// MCP tool catalog.
pub fn tools() -> Vec<Value> {
    let amount = json!({ "type": "string", "description": "DE amount, signed: \"1.200,00\" debit, \"-1.000,00\" credit. Or use amount_cents (integer)." });
    let line = json!({
        "type": "object",
        "properties": {
            "account": { "type": "integer" },
            "amount": amount,
            "amount_cents": { "type": "integer" },
            "tax_key": { "type": "string", "description": "U20 U10 U13 V20 V10 V13 IGE20. Amount is then NET; the engine adds the tax line." },
            "memo": { "type": "string" }
        },
        "required": ["account"]
    });
    let range = Value::Object(date_props(&["from", "to"]));
    vec![
        tool("buch_status", "Ledger status: counts, open drafts, unmatched bank lines, closed periods, demo flag.", json!({}), &[]),
        tool("buch_accounts_list", "List the chart of accounts (EKR).", json!({}), &[]),
        tool("buch_accounts_seed", "Insert the EKR seed chart (existing numbers untouched).", json!({}), &[]),
        tool("buch_account_add", "Add an account.", json!({ "number": {"type":"integer"}, "name": {"type":"string"}, "kind": {"type":"string","enum":["asset","liability","equity","revenue","expense"]}, "tax_key": {"type":"string"} }), &["number", "name", "kind"]),
        tool("buch_entry_book", "Book a balanced journal entry. Net lines carry a tax_key; the engine generates the tax lines. Lines (incl. generated tax) must sum to zero.", json!({ "date": {"type":"string"}, "kreis": {"type":"string","enum":["AR","ER","BK","KA","SO"]}, "description": {"type":"string"}, "beleg_nr": {"type":"string"}, "lines": {"type":"array","items": line} }), &["date", "kreis", "description", "lines"]),
        tool("buch_entry_book_gross", "Book a gross amount in one step: account gets net, engine adds tax, contra gets -gross.", json!({ "date": {"type":"string"}, "kreis": {"type":"string"}, "description": {"type":"string"}, "account": {"type":"integer"}, "contra": {"type":"integer"}, "gross": amount, "gross_cents": {"type":"integer"}, "tax_key": {"type":"string"}, "beleg_nr": {"type":"string"} }), &["date", "kreis", "description", "account", "contra"]),
        tool("buch_entry_reverse", "Storno an entry (the only way to correct a booking).", json!({ "id": {"type":"integer"}, "date": {"type":"string"}, "reason": {"type":"string"} }), &["id", "date", "reason"]),
        tool("buch_entry_show", "Show one entry with its lines.", json!({ "id": {"type":"integer"} }), &["id"]),
        tool("buch_journal", "Journal for a date range, optionally one Belegkreis.", json!({ "from": {"type":"string"}, "to": {"type":"string"}, "kreis": {"type":"string"}, "limit": {"type":"integer"} }), &["from", "to"]),
        tool("buch_balance", "Saldenliste (debit, credit, balance per account) for a date range.", range, &["from", "to"]),
        tool("buch_account_ledger", "Kontoblatt with opening and running balance.", json!({ "account": {"type":"integer"}, "from": {"type":"string"}, "to": {"type":"string"} }), &["account", "from", "to"]),
        tool("buch_u30", "U30 VAT pre-fill (Kennzahlen 000/006/022/029/060/065/070/072/095) for a month YYYY-MM. Not a filing.", json!({ "month": {"type":"string"} }), &["month"]),
        tool("buch_period_close", "Close a month (YYYY-MM); later bookings dated in it are refused.", json!({ "month": {"type":"string"} }), &["month"]),
        tool("buch_bank_import", "Import a bank statement CSV (German or English headers). Re-importing the same file adds nothing. Needs --allow-path.", json!({ "path": {"type":"string"} }), &["path"]),
        tool("buch_bank_unmatched", "Bank lines not yet booked.", json!({ "limit": {"type":"integer"} }), &[]),
        tool("buch_bank_book", "Book an unmatched bank line against an account (gross; tax key splits it) and link it.", json!({ "id": {"type":"integer"}, "account": {"type":"integer"}, "tax_key": {"type":"string"}, "description": {"type":"string"} }), &["id", "account"]),
        tool("buch_drafts", "Drafts created from other apps' events (open only unless all=true).", json!({ "all": {"type":"boolean"} }), &[]),
        tool("buch_draft_approve", "Book a draft.", json!({ "id": {"type":"integer"} }), &["id"]),
        tool("buch_draft_discard", "Discard a draft.", json!({ "id": {"type":"integer"} }), &["id"]),
        tool("buch_inbox_drain", "Read events from other craft apps (e.g. assetcraft depreciation) and turn them into drafts.", json!({}), &[]),
        tool("buch_export_csv", "Export journal lines as semicolon CSV (DE format). Needs --allow-path.", json!({ "from": {"type":"string"}, "to": {"type":"string"}, "path": {"type":"string"} }), &["from", "to", "path"]),
        tool("buch_demo_seed", "Fill an EMPTY ledger with a labelled demo company (October 2026).", json!({}), &[]),
    ]
}

#[cfg(test)]
mod tests;
