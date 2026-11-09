//! `buchcraft-app`: the BuchCraft desktop window.
//!
//! Flags: `--db PATH`, `--lang de|en`, `--dark`, `--view NAME`, `--month YYYY-MM`,
//! `--smoke` (headless status check, no window), `--screenshot PATH` (render,
//! save one PNG, exit).
#![deny(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

mod app;

use app::{BuchApp, MonthSel, Shot, View};
use buchcraft_core::{dispatch, Command, Confinement, Ctx};
use buchcraft_store::Ledger;
use craft_biz::bus::Bus;
use craft_biz::i18n::Locale;
use eframe::egui;
use std::path::PathBuf;
use std::process::ExitCode;

#[derive(Default)]
struct Args {
    db: Option<PathBuf>,
    lang: Option<String>,
    dark: bool,
    view: Option<String>,
    month: Option<String>,
    smoke: bool,
    screenshot: Option<PathBuf>,
}

fn parse_args() -> Result<Args, String> {
    let mut a = Args::default();
    let mut it = std::env::args().skip(1);
    while let Some(arg) = it.next() {
        let mut val = |name: &str| it.next().ok_or_else(|| format!("{name} needs a value"));
        match arg.as_str() {
            "--db" => a.db = Some(PathBuf::from(val("--db")?)),
            "--lang" => a.lang = Some(val("--lang")?),
            "--dark" => a.dark = true,
            "--view" => a.view = Some(val("--view")?),
            "--month" => a.month = Some(val("--month")?),
            "--smoke" => a.smoke = true,
            "--screenshot" => a.screenshot = Some(PathBuf::from(val("--screenshot")?)),
            other => return Err(format!("unknown argument {other}")),
        }
    }
    Ok(a)
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

fn main() -> ExitCode {
    let args = match parse_args() {
        Ok(a) => a,
        Err(e) => {
            eprintln!("buchcraft-app: {e}");
            return ExitCode::FAILURE;
        }
    };
    let db = args.db.clone().unwrap_or_else(default_db);
    let ledger = match Ledger::open(&db) {
        Ok(l) => l,
        Err(e) => {
            eprintln!("buchcraft-app: cannot open {}: {e}", db.display());
            return ExitCode::FAILURE;
        }
    };
    let mut ctx = Ctx {
        ledger,
        confinement: Confinement(None),
        bus: Bus::new(Bus::default_root()),
    };
    if args.smoke {
        return match dispatch(&mut ctx, Command::Status) {
            Ok(v) => {
                println!("{v}");
                ExitCode::SUCCESS
            }
            Err(e) => {
                eprintln!("buchcraft-app smoke: {e}");
                ExitCode::FAILURE
            }
        };
    }
    let mut app = BuchApp::new(ctx, db.display().to_string());
    if let Some(l) = args.lang.as_deref().and_then(Locale::parse) {
        app.locale = l;
    }
    app.dark = args.dark;
    if let Some(v) = args.view.as_deref().and_then(View::parse) {
        app.view = v;
    }
    if let Some(m) = args.month.as_deref().and_then(MonthSel::parse) {
        app.month = m;
        app.reload();
    }
    app.shot = args.screenshot.map(Shot::new);

    let options = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default()
            .with_inner_size([1280.0, 820.0])
            .with_min_inner_size([960.0, 600.0])
            .with_title("BuchCraft"),
        ..Default::default()
    };
    match eframe::run_native("BuchCraft", options, Box::new(move |_cc| Ok(Box::new(app)))) {
        Ok(()) => ExitCode::SUCCESS,
        Err(e) => {
            eprintln!("buchcraft-app: {e}");
            ExitCode::FAILURE
        }
    }
}
