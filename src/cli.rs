//! Headless mode, also handy for scripting and for verifying the hook engine.

use crate::backup::BackupStore;
use crate::launcher::{self, HackOptions, Level, LogLine};
use crate::target::Target;
use crate::util;
use std::path::PathBuf;

const HELP: &str = r#"LauncherLauncher // 启动器启动器

GUI:
  launcherlauncher

Headless:
  launcherlauncher --hack <path> [--no-updates] [--no-launch] [--dry-run]
                                  [--rules <json> | --rules-file <file>]
  launcherlauncher --inspect <path>
  launcherlauncher --list
  launcherlauncher --restore <backup-id>
  launcherlauncher --version
  launcherlauncher --help

Options:
  --no-updates   keep the launcher's update checker untouched
  --no-launch    patch but do not start the launcher
  --dry-run      scan and report, write nothing
  --rules        inline JSON rule array (see README)
  --rules-file   read the JSON rule array from a file
"#;

pub fn run(args: &[String]) -> i32 {
    let Some(cmd) = args.first().map(String::as_str) else {
        print!("{HELP}");
        return 0;
    };
    match cmd {
        "--help" | "-h" | "help" => {
            print!("{HELP}");
            0
        }
        "--hack" => cmd_hack(&args[1..]),
        "--inspect" => cmd_inspect(&args[1..]),
        "--list" => cmd_list(),
        "--restore" => cmd_restore(&args[1..]),
        "--version" | "-V" => {
            println!("launcherlauncher {}", env!("CARGO_PKG_VERSION"));
            0
        }
        other => {
            eprintln!("未知命令: {other}\n\n{HELP}");
            2
        }
    }
}

fn print_log(l: LogLine) {
    let tag = match l.level {
        Level::Info => ">>",
        Level::Ok => "OK",
        Level::Warn => "!!",
        Level::Error => "XX",
    };
    println!("{tag} {}", l.text);
}

fn cmd_hack(args: &[String]) -> i32 {
    let mut path: Option<PathBuf> = None;
    let mut remove_updates = true;
    let mut launch = true;
    let mut dry_run = false;
    let mut rules = String::new();
    let mut i = 0;
    while i < args.len() {
        match args[i].as_str() {
            "--no-updates" => remove_updates = false,
            "--no-launch" => launch = false,
            "--dry-run" => dry_run = true,
            "--rules" => {
                i += 1;
                match args.get(i) {
                    Some(v) => rules = v.clone(),
                    None => {
                        eprintln!("--rules 需要一个 JSON 参数");
                        return 2;
                    }
                }
            }
            "--rules-file" => {
                i += 1;
                match args.get(i) {
                    Some(v) => match std::fs::read_to_string(v) {
                        Ok(s) => rules = s,
                        Err(e) => {
                            eprintln!("读取规则文件失败: {e}");
                            return 1;
                        }
                    },
                    None => {
                        eprintln!("--rules-file 需要一个文件路径");
                        return 2;
                    }
                }
            }
            other if !other.starts_with('-') => path = Some(PathBuf::from(other)),
            other => {
                eprintln!("未知参数: {other}");
                return 2;
            }
        }
        i += 1;
    }
    let Some(path) = path else {
        eprintln!("用法: launcherlauncher --hack <path> [options]");
        return 2;
    };
    let opts = HackOptions {
        remove_updates,
        custom_rules: rules,
        launch,
        dry_run,
    };
    match launcher::hack(&path, &opts, &mut print_log) {
        Ok(outcome) => {
            println!();
            println!(
                "target : {} {}",
                outcome.target.label, outcome.target.version
            );
            println!("applied: {} rule(s)", outcome.report.applied.len());
            println!(
                "result : {}",
                if outcome.changed {
                    "patched"
                } else if outcome.already_patched {
                    "already patched"
                } else {
                    "unchanged"
                }
            );
            if let Some(b) = &outcome.backup {
                println!("backup : {} ({})", b.id, b.dir().display());
            }
            if let Some(pid) = outcome.pid {
                println!("pid    : {pid}");
            }
            0
        }
        Err(e) => {
            eprintln!("XX {e:#}");
            1
        }
    }
}

fn cmd_inspect(args: &[String]) -> i32 {
    let Some(path) = args.first() else {
        eprintln!("用法: launcherlauncher --inspect <path>");
        return 2;
    };
    match Target::detect(std::path::Path::new(path)) {
        Ok(t) => {
            println!("label      : {}", t.label);
            println!("version    : {}", t.version);
            println!("kind       : {}", t.kind.label());
            println!("size       : {}", util::human_bytes(t.bytes));
            println!("fingerprint: {}", t.fingerprint);
            println!("selected   : {}", t.path.display());
            println!("primary    : {}", t.primary.display());
            0
        }
        Err(e) => {
            eprintln!("XX {e:#}");
            1
        }
    }
}

fn cmd_list() -> i32 {
    let backups = BackupStore::list();
    if backups.is_empty() {
        println!("没有备份。");
        return 0;
    }
    for b in backups {
        println!(
            "{}\t{} {}\t{}\t{}\t{}",
            b.id,
            b.label,
            b.version,
            util::fmt_ts(b.created_at),
            util::human_bytes(b.size),
            util::short_hash(&b.fingerprint)
        );
    }
    0
}

fn cmd_restore(args: &[String]) -> i32 {
    let Some(id) = args.first() else {
        eprintln!("用法: launcherlauncher --restore <backup-id>（用 --list 查看）");
        return 2;
    };
    let Some(entry) = BackupStore::list().into_iter().find(|b| &b.id == id) else {
        eprintln!("找不到备份: {id}");
        return 1;
    };
    match launcher::restore(&entry, &mut print_log) {
        Ok(()) => 0,
        Err(e) => {
            eprintln!("XX {e:#}");
            1
        }
    }
}
