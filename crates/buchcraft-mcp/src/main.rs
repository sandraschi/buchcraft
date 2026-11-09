//! `buchcraft`: CLI and MCP stdio server in one binary.
//!
//! Every CLI verb builds the same tool-argument JSON an MCP client would send
//! and goes through `Command::from_tool` + `dispatch`, so CLI and MCP share one
//! code path. In `mcp` mode stdout carries only JSON-RPC; logs go to stderr.
#![deny(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use buchcraft_core::{dispatch, tools, Command, Confinement, Ctx, APP, VERSION};
use buchcraft_store::Ledger;
use clap::{Parser, Subcommand};
use craft_biz::bus::Bus;
use craft_biz::mcp::{serve_stdio, ToolHost};
use serde_json::{json, Map, Value};
use std::path::PathBuf;
use std::process::ExitCode;

const AFTER_HELP: &str = "Typical month:
  buchcraft gross --date 05.11.2026 --kreis ER --account 7600 --contra 2800 --gross \"120,00\" --tax-key V20 \"Büromaterial\"
  buchcraft bank import D:\\Bank\\george.csv      # then: bank unmatched, bank book
  buchcraft inbox-drain; buchcraft drafts list  # AfA drafts from assetcraft
  buchcraft u30 2026-11                         # UVA pre-fill
  buchcraft balance --from 2026-01-01 --to 2026-11-30
  buchcraft close-month 2026-11
See HELP.md for workflows and docs/TOOLS.md for every MCP tool.";

#[derive(Parser)]
#[command(after_help = AFTER_HELP,
    name = "buchcraft",
    version,
    about = "Austrian double-entry bookkeeping (EKR, U30 pre-fill), local-first. CLI + MCP."
)]
struct Cli {
    /// Ledger database. Default: $BUCHCRAFT_DB or %LOCALAPPDATA%\buchcraft\buchcraft.db
    #[arg(long, global = true)]
    db: Option<PathBuf>,
    /// Print machine-readable JSON only.
    #[arg(long, global = true)]
    json: bool,
    /// Restrict file access to these directories (repeatable). Required for file tools in `mcp` mode.
    #[arg(long = "allow-path", global = true)]
    allow_path: Vec<PathBuf>,
    #[command(subcommand)]
    cmd: Cmd,
}

#[derive(Subcommand)]
enum Cmd {
    /// Ledger status and counts.
    Status,
    /// Chart of accounts.
    Accounts {
        #[command(subcommand)]
        op: AccountsOp,
    },
    /// Book an entry from a JSON file or string (same shape as the buch_entry_book tool).
    Book {
        /// JSON with date, kreis, description, lines[]; prefix with @ to read a file.
        entry: String,
    },
    /// Book a gross amount in one line: account gets net, tax line added, contra gets -gross.
    Gross {
        #[arg(long)]
        date: String,
        #[arg(long)]
        kreis: String,
        #[arg(long)]
        account: u32,
        #[arg(long)]
        contra: u32,
        /// DE amount, e.g. "1.200,00" (negative for credit on account)
        #[arg(long, allow_hyphen_values = true)]
        gross: String,
        #[arg(long)]
        tax_key: Option<String>,
        #[arg(long)]
        beleg_nr: Option<String>,
        description: String,
    },
    /// Storno an entry.
    Reverse {
        id: i64,
        #[arg(long)]
        date: String,
        #[arg(long)]
        reason: String,
    },
    /// Show one entry.
    Show { id: i64 },
    /// Journal for a date range.
    Journal {
        #[arg(long)]
        from: String,
        #[arg(long)]
        to: String,
        #[arg(long)]
        kreis: Option<String>,
        #[arg(long, default_value_t = 200)]
        limit: u64,
    },
    /// Saldenliste.
    Balance {
        #[arg(long)]
        from: String,
        #[arg(long)]
        to: String,
    },
    /// Kontoblatt for one account.
    Ledger {
        account: u32,
        #[arg(long)]
        from: String,
        #[arg(long)]
        to: String,
    },
    /// U30 VAT pre-fill for YYYY-MM.
    U30 { month: String },
    /// Close a month (YYYY-MM).
    CloseMonth { month: String },
    /// Bank statements.
    Bank {
        #[command(subcommand)]
        op: BankOp,
    },
    /// Drafts from other craft apps.
    Drafts {
        #[command(subcommand)]
        op: DraftsOp,
    },
    /// Read events from other craft apps into drafts.
    InboxDrain,
    /// Export journal lines as semicolon CSV.
    Export {
        #[arg(long)]
        from: String,
        #[arg(long)]
        to: String,
        path: String,
    },
    /// Fill an empty ledger with the labelled demo company.
    DemoSeed,
    /// Print the MCP tool catalog (JSON, or markdown for docs/TOOLS.md).
    Tools {
        #[arg(long)]
        markdown: bool,
    },
    /// Run the MCP stdio server.
    Mcp,
}

#[derive(Subcommand)]
enum AccountsOp {
    List,
    Seed,
    Add {
        number: u32,
        name: String,
        #[arg(long)]
        kind: String,
        #[arg(long)]
        tax_key: Option<String>,
    },
}

#[derive(Subcommand)]
enum BankOp {
    Import {
        path: String,
    },
    Unmatched {
        #[arg(long, default_value_t = 200)]
        limit: u64,
    },
    Book {
        id: i64,
        #[arg(long)]
        account: u32,
        #[arg(long)]
        tax_key: Option<String>,
        #[arg(long)]
        description: Option<String>,
    },
}

#[derive(Subcommand)]
enum DraftsOp {
    List {
        #[arg(long)]
        all: bool,
    },
    Approve {
        id: i64,
    },
    Discard {
        id: i64,
    },
}

fn default_db() -> PathBuf {
    if let Ok(p) = std::env::var("BUCHCRAFT_DB") {
        if !p.trim().is_empty() {
            return PathBuf::from(p);
        }
    }
    let base = std::env::var("LOCALAPPDATA")
        .map(PathBuf::from)
        .unwrap_or_else(|_| PathBuf::from("."));
    base.join("buchcraft").join("buchcraft.db")
}

/// Build a JSON object, dropping `None` values.
fn obj(pairs: Vec<(&str, Option<Value>)>) -> Value {
    let mut m = Map::new();
    for (k, v) in pairs {
        if let Some(v) = v {
            m.insert(k.to_string(), v);
        }
    }
    Value::Object(m)
}

fn sv(s: &str) -> Option<Value> {
    Some(Value::String(s.to_string()))
}

/// Map a CLI verb onto (tool name, arguments).
fn to_tool(cmd: Cmd) -> Result<(&'static str, Value), String> {
    Ok(match cmd {
        Cmd::Status => ("buch_status", json!({})),
        Cmd::Accounts {
            op: AccountsOp::List,
        } => ("buch_accounts_list", json!({})),
        Cmd::Accounts {
            op: AccountsOp::Seed,
        } => ("buch_accounts_seed", json!({})),
        Cmd::Accounts {
            op:
                AccountsOp::Add {
                    number,
                    name,
                    kind,
                    tax_key,
                },
        } => (
            "buch_account_add",
            obj(vec![
                ("number", Some(json!(number))),
                ("name", sv(&name)),
                ("kind", sv(&kind)),
                ("tax_key", tax_key.map(Value::String)),
            ]),
        ),
        Cmd::Book { entry } => {
            let text = match entry.strip_prefix('@') {
                Some(path) => std::fs::read_to_string(path).map_err(|e| format!("{path}: {e}"))?,
                None => entry,
            };
            (
                "buch_entry_book",
                serde_json::from_str(&text).map_err(|e| format!("entry JSON: {e}"))?,
            )
        }
        Cmd::Gross {
            date,
            kreis,
            account,
            contra,
            gross,
            tax_key,
            beleg_nr,
            description,
        } => (
            "buch_entry_book_gross",
            obj(vec![
                ("date", sv(&date)),
                ("kreis", sv(&kreis)),
                ("account", Some(json!(account))),
                ("contra", Some(json!(contra))),
                ("gross", sv(&gross)),
                ("tax_key", tax_key.map(Value::String)),
                ("beleg_nr", beleg_nr.map(Value::String)),
                ("description", sv(&description)),
            ]),
        ),
        Cmd::Reverse { id, date, reason } => (
            "buch_entry_reverse",
            json!({ "id": id, "date": date, "reason": reason }),
        ),
        Cmd::Show { id } => ("buch_entry_show", json!({ "id": id })),
        Cmd::Journal {
            from,
            to,
            kreis,
            limit,
        } => (
            "buch_journal",
            obj(vec![
                ("from", sv(&from)),
                ("to", sv(&to)),
                ("kreis", kreis.map(Value::String)),
                ("limit", Some(json!(limit))),
            ]),
        ),
        Cmd::Balance { from, to } => ("buch_balance", json!({ "from": from, "to": to })),
        Cmd::Ledger { account, from, to } => (
            "buch_account_ledger",
            json!({ "account": account, "from": from, "to": to }),
        ),
        Cmd::U30 { month } => ("buch_u30", json!({ "month": month })),
        Cmd::CloseMonth { month } => ("buch_period_close", json!({ "month": month })),
        Cmd::Bank {
            op: BankOp::Import { path },
        } => ("buch_bank_import", json!({ "path": path })),
        Cmd::Bank {
            op: BankOp::Unmatched { limit },
        } => ("buch_bank_unmatched", json!({ "limit": limit })),
        Cmd::Bank {
            op:
                BankOp::Book {
                    id,
                    account,
                    tax_key,
                    description,
                },
        } => (
            "buch_bank_book",
            obj(vec![
                ("id", Some(json!(id))),
                ("account", Some(json!(account))),
                ("tax_key", tax_key.map(Value::String)),
                ("description", description.map(Value::String)),
            ]),
        ),
        Cmd::Drafts {
            op: DraftsOp::List { all },
        } => ("buch_drafts", json!({ "all": all })),
        Cmd::Drafts {
            op: DraftsOp::Approve { id },
        } => ("buch_draft_approve", json!({ "id": id })),
        Cmd::Drafts {
            op: DraftsOp::Discard { id },
        } => ("buch_draft_discard", json!({ "id": id })),
        Cmd::InboxDrain => ("buch_inbox_drain", json!({})),
        Cmd::Export { from, to, path } => (
            "buch_export_csv",
            json!({ "from": from, "to": to, "path": path }),
        ),
        Cmd::DemoSeed => ("buch_demo_seed", json!({})),
        Cmd::Tools { .. } | Cmd::Mcp => return Err("not a tool verb".into()),
    })
}

struct Host {
    ctx: Ctx,
}

impl ToolHost for Host {
    fn server_name(&self) -> &str {
        APP
    }
    fn server_version(&self) -> &str {
        VERSION
    }
    fn tools(&self) -> Vec<Value> {
        tools()
    }
    fn call(&mut self, name: &str, args: &Value) -> Result<Value, String> {
        let cmd = Command::from_tool(name, args)?;
        dispatch(&mut self.ctx, cmd)
    }
}

/// Human rendering: tables for row lists, indented JSON otherwise.
fn render(v: &Value) -> String {
    for key in ["rows", "entries", "accounts", "transactions", "drafts"] {
        if let Some(rows) = v.get(key).and_then(Value::as_array) {
            let mut out = String::new();
            for (k, val) in v.as_object().into_iter().flatten() {
                if k != key && !val.is_array() && !val.is_object() && !has_de(v, k) {
                    out.push_str(&format!("{k}: {}\n", scalar(val)));
                }
            }
            for row in rows {
                let cells: Vec<String> = row
                    .as_object()
                    .into_iter()
                    .flatten()
                    .filter(|(k, val)| {
                        !val.is_array()
                            && !val.is_object()
                            && !k.ends_with("_cents")
                            && *k != "amount"
                            && *k != "running"
                            && *k != "saldo"
                            && *k != "debit"
                            && *k != "credit"
                    })
                    .map(|(_, val)| scalar(val))
                    .collect();
                out.push_str(&cells.join("  |  "));
                out.push('\n');
            }
            if rows.is_empty() {
                out.push_str("(none)\n");
            }
            return out;
        }
    }
    serde_json::to_string_pretty(v).unwrap_or_else(|e| e.to_string())
}

/// Skip a raw cents field when a formatted <k>_de sibling exists.
fn has_de(obj: &Value, k: &str) -> bool {
    obj.get(format!("{k}_de")).is_some()
}

fn scalar(v: &Value) -> String {
    match v {
        Value::String(s) => s.clone(),
        Value::Null => "-".into(),
        other => other.to_string(),
    }
}

fn main() -> ExitCode {
    let cli = Cli::parse();
    if let Cmd::Tools { markdown } = cli.cmd {
        if markdown {
            print!("{}", craft_biz::mcp::tools_markdown(APP, &tools()));
            return ExitCode::SUCCESS;
        }
        println!(
            "{}",
            serde_json::to_string_pretty(&tools()).unwrap_or_default()
        );
        return ExitCode::SUCCESS;
    }
    let db = cli.db.clone().unwrap_or_else(default_db);
    let ledger = match Ledger::open(&db) {
        Ok(l) => l,
        Err(e) => {
            eprintln!("buchcraft: cannot open {}: {e}", db.display());
            return ExitCode::FAILURE;
        }
    };
    let is_mcp = matches!(cli.cmd, Cmd::Mcp);
    let confinement = if is_mcp || !cli.allow_path.is_empty() {
        match Confinement::roots(&cli.allow_path) {
            Ok(c) => c,
            Err(e) => {
                eprintln!("buchcraft: {e}");
                return ExitCode::FAILURE;
            }
        }
    } else {
        Confinement(None)
    };
    let ctx = Ctx {
        ledger,
        confinement,
        bus: Bus::new(Bus::default_root()),
    };
    if is_mcp {
        eprintln!("[buchcraft] db {}", db.display());
        let mut host = Host { ctx };
        return match serve_stdio(&mut host) {
            Ok(()) => ExitCode::SUCCESS,
            Err(e) => {
                eprintln!("buchcraft mcp: {e}");
                ExitCode::FAILURE
            }
        };
    }
    let json_out = cli.json;
    let mut ctx = ctx;
    let result = to_tool(cli.cmd)
        .and_then(|(name, args)| Command::from_tool(name, &args))
        .and_then(|cmd| dispatch(&mut ctx, cmd));
    match result {
        Ok(v) => {
            if json_out {
                println!("{}", serde_json::to_string_pretty(&v).unwrap_or_default());
            } else {
                print!("{}", render(&v));
            }
            ExitCode::SUCCESS
        }
        Err(e) => {
            if json_out {
                println!("{}", json!({ "error": e }));
            } else {
                eprintln!("buchcraft: {e}");
            }
            ExitCode::FAILURE
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use craft_biz::mcp::handle_line;

    #[test]
    fn tools_doc_is_current() {
        let committed = include_str!("../../../docs/TOOLS.md");
        assert!(
            craft_biz::mcp::docs_in_sync(committed, &craft_biz::mcp::tools_markdown(APP, &tools())),
            "docs/TOOLS.md is stale: run `just docs`"
        );
    }

    fn host() -> Host {
        let ledger = Ledger::open_in_memory().unwrap();
        let bus = Bus::new(
            std::env::temp_dir().join(format!("buchcraft-mcp-test-{}", std::process::id())),
        );
        Host {
            ctx: Ctx {
                ledger,
                confinement: Confinement(Some(Vec::new())),
                bus,
            },
        }
    }

    #[test]
    fn mcp_probe_initialize_list_call() {
        let mut h = host();
        let init = handle_line(
            &mut h,
            r#"{"jsonrpc":"2.0","id":1,"method":"initialize","params":{}}"#,
        )
        .unwrap();
        assert_eq!(init["result"]["serverInfo"]["name"], "buchcraft");
        let list =
            handle_line(&mut h, r#"{"jsonrpc":"2.0","id":2,"method":"tools/list"}"#).unwrap();
        assert!(
            list["result"]["tools"]
                .as_array()
                .map(Vec::len)
                .unwrap_or(0)
                >= 20
        );
        let seed = handle_line(&mut h, r#"{"jsonrpc":"2.0","id":3,"method":"tools/call","params":{"name":"buch_demo_seed","arguments":{}}}"#).unwrap();
        assert_eq!(seed["result"]["isError"], false);
        let u30 = handle_line(&mut h, r#"{"jsonrpc":"2.0","id":4,"method":"tools/call","params":{"name":"buch.u30","arguments":{"month":"2026-10"}}}"#).unwrap();
        let text = u30["result"]["content"][0]["text"].as_str().unwrap();
        assert!(text.contains("\"095\""));
        let bad = handle_line(&mut h, r#"{"jsonrpc":"2.0","id":5,"method":"tools/call","params":{"name":"buch_bank_import","arguments":{"path":"C:/Windows/win.ini"}}}"#).unwrap();
        assert_eq!(bad["result"]["isError"], true);
    }

    #[test]
    fn cli_verbs_map_to_known_tools() {
        let verbs = vec![
            Cmd::Status,
            Cmd::Accounts {
                op: AccountsOp::List,
            },
            Cmd::U30 {
                month: "2026-10".into(),
            },
            Cmd::Bank {
                op: BankOp::Unmatched { limit: 5 },
            },
            Cmd::Drafts {
                op: DraftsOp::List { all: true },
            },
            Cmd::DemoSeed,
        ];
        let names: Vec<String> = tools()
            .iter()
            .filter_map(|t| t["name"].as_str().map(str::to_string))
            .collect();
        for v in verbs {
            let (name, _) = to_tool(v).unwrap();
            assert!(names.iter().any(|n| n == name), "{name}");
        }
    }

    #[test]
    fn render_tables_without_panicking() {
        let v = json!({ "from": "2026-10-01", "rows": [ { "account": 2000, "name": "Forderungen", "saldo_de": "1.200,00" } ] });
        let s = render(&v);
        assert!(s.contains("Forderungen"));
        assert!(render(&json!({ "rows": [] })).contains("(none)"));
    }
}
