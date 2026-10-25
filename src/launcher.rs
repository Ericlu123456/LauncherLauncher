//! The "Hack" pipeline: detect -> back up -> patch -> launch.

use crate::backup::{BackupEntry, BackupStore, PatchRecord, State};
use crate::patch::{self, Report, Rule, profiles};
use crate::paths;
use crate::target::Target;
use crate::util;
use anyhow::{Context, Result, bail};
use std::fs;
use std::path::Path;
use std::process::{Command, Stdio};
use std::time::Duration;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Level {
    Info,
    Ok,
    Warn,
    Error,
}

#[derive(Clone, Debug)]
pub struct LogLine {
    pub level: Level,
    pub text: String,
}

impl LogLine {
    pub fn info(text: impl Into<String>) -> Self {
        Self {
            level: Level::Info,
            text: text.into(),
        }
    }
    pub fn ok(text: impl Into<String>) -> Self {
        Self {
            level: Level::Ok,
            text: text.into(),
        }
    }
    pub fn warn(text: impl Into<String>) -> Self {
        Self {
            level: Level::Warn,
            text: text.into(),
        }
    }
    pub fn error(text: impl Into<String>) -> Self {
        Self {
            level: Level::Error,
            text: text.into(),
        }
    }
}

pub struct HackOptions {
    pub remove_updates: bool,
    pub custom_rules: String,
    pub launch: bool,
    pub dry_run: bool,
}

impl Default for HackOptions {
    fn default() -> Self {
        Self {
            remove_updates: true,
            custom_rules: String::new(),
            launch: true,
            dry_run: false,
        }
    }
}

#[derive(Debug)]
pub struct HackOutcome {
    pub target: Target,
    pub backup: Option<BackupEntry>,
    pub report: Report,
    pub changed: bool,
    pub patched_fingerprint: Option<String>,
    pub update_detected: bool,
    pub already_patched: bool,
    pub pid: Option<u32>,
}

/// PCL2 2.99.0.1 removed the download entries upstream. We keep the
/// well-tested 2.99.0.0 line and refuse newer builds instead of pretending
/// that a loader-only patch restores the page.
fn parse_version(version: &str) -> Option<(u64, u64, u64, u64)> {
    let core = version
        .trim()
        .trim_start_matches(['v', 'V'])
        .split(['-', '+'])
        .next()?
        .trim();
    let mut out = [0u64; 4];
    let mut count = 0usize;
    for part in core.split('.') {
        if part.is_empty() {
            break;
        }
        out[count] = part.parse().ok()?;
        count += 1;
        if count == 4 {
            break;
        }
    }
    if count == 0 {
        None
    } else {
        Some((out[0], out[1], out[2], out[3]))
    }
}

fn ensure_supported(target: &Target) -> Result<()> {
    if target.label != "PCL2" {
        return Ok(());
    }
    match parse_version(&target.version) {
        Some(v) if v <= (2, 99, 0, 0) => Ok(()),
        Some(_) => bail!(
            "PCL2 {} 暂不支持：上游 2.99.0.1 起已移除下载入口；本项目只支持 2.99.0.0 及以下。",
            target.version
        ),
        None => bail!(
            "无法识别 PCL2 版本（{}）；为避免误改，只支持 2.99.0.0 及以下。",
            target.version
        ),
    }
}

pub fn hack(path: &Path, opts: &HackOptions, log: &mut dyn FnMut(LogLine)) -> Result<HackOutcome> {
    paths::ensure_dirs()?;
    log(LogLine::info("正在识别启动器…"));
    let target = Target::detect(path)?;
    ensure_supported(&target)?;
    log(LogLine::info(format!(
        "识别为 {} {} · {} · {} · {}",
        target.label,
        target.version,
        target.kind.label(),
        util::human_bytes(target.bytes),
        util::short_hash(&target.fingerprint)
    )));

    let mut state = State::load();
    let current_fp = target.fingerprint.clone();
    let mut update_detected = false;

    if let Some(last) = &state.last_path {
        if last == &target.path
            && state.last_patched_fingerprint.as_deref() != Some(current_fp.as_str())
            && state
                .last_original_fingerprint
                .as_deref()
                .map(|f| f != current_fp)
                .unwrap_or(false)
        {
            update_detected = true;
            log(LogLine::warn(format!(
                "检测到启动器更新：{} → {}",
                state.last_version.as_deref().unwrap_or("unknown"),
                target.version
            )));
            if let Some(id) = &state.last_backup_id {
                if let Some(old) = BackupStore::list().into_iter().find(|b| &b.id == id) {
                    log(LogLine::ok(format!(
                        "旧版已自动归档：{}（{}）",
                        old.id,
                        util::fmt_ts(old.created_at)
                    )));
                }
            }
        }
    }

    let mut rules: Vec<Rule> = profiles::for_launcher(&target.label, opts.remove_updates);
    let builtin = rules.len();
    match patch::parse_custom_rules(&opts.custom_rules) {
        Ok(custom) => rules.extend(custom.iter().cloned()),
        Err(e) => log(LogLine::warn(format!("自定义规则解析失败，已忽略：{e}"))),
    }
    log(LogLine::info(format!(
        "规则：内置 {builtin} 条 + 自定义 {} 条{}",
        rules.len().saturating_sub(builtin),
        if opts.remove_updates {
            "（同时移除更新检查）"
        } else {
            ""
        }
    )));
    let rules_json = serde_json::to_vec(&rules).unwrap_or_default();
    let rules_hash = util::sha256_hex(&rules_json);

    // If the file is already a patched output (state lost, or a previous run),
    // don't treat the patched bytes as the new "original".
    let mut already_patched =
        state.last_patched_fingerprint.as_deref() == Some(current_fp.as_str());
    let mut recovered_source: Option<String> = None;
    if !already_patched {
        if let Some(record) = PatchRecord::load_for_output(&current_fp) {
            if BackupStore::find_by_fingerprint(&record.source_fingerprint).is_some() {
                already_patched = true;
                recovered_source = Some(record.source_fingerprint.clone());
                log(LogLine::info(
                    "检测到该文件已经是 Hook 后的版本（从修补记录恢复状态）",
                ));
            }
        }
    }

    let backup = if opts.dry_run && !already_patched {
        log(LogLine::info("预览模式：跳过备份（不会写入启动器）"));
        None
    } else if already_patched {
        let want = recovered_source
            .clone()
            .or_else(|| state.last_original_fingerprint.clone());
        let found = want
            .as_deref()
            .and_then(BackupStore::find_by_fingerprint)
            .or_else(|| {
                state
                    .last_backup_id
                    .as_ref()
                    .and_then(|id| BackupStore::list().into_iter().find(|b| &b.id == id))
            });
        if let Some(b) = found {
            log(LogLine::info(format!("复用已有备份：{}", b.id)));
            Some(b)
        } else if opts.dry_run {
            log(LogLine::warn("找不到原始备份；预览模式不创建新备份"));
            None
        } else {
            log(LogLine::warn("找不到原始备份，重新备份当前（已 Hook）文件"));
            Some(BackupStore::ensure(&target)?)
        }
    } else {
        let b = BackupStore::ensure(&target)?;
        log(LogLine::ok(format!(
            "已备份原始启动器：{}（{}）",
            b.id,
            util::human_bytes(b.size)
        )));
        Some(b)
    };

    let current_record = PatchRecord::load_for_output(&current_fp);
    let prev_rules_hash = state
        .last_rules_hash
        .clone()
        .or_else(|| current_record.as_ref().and_then(|r| r.rules_hash.clone()));
    let rules_changed = already_patched && prev_rules_hash.as_deref() != Some(rules_hash.as_str());
    let re_patch = rules_changed && !rules.is_empty();
    if re_patch {
        let original_available = backup
            .as_ref()
            .map(|b| b.fingerprint != current_fp)
            .unwrap_or(false);
        if original_available {
            log(LogLine::ok("Hook 规则已变化：从原始备份重新应用"));
        } else {
            log(LogLine::warn(
                "Hook 规则已变化，但原始备份不可用；将在当前文件上叠加规则",
            ));
        }
    }
    let source_fp = if re_patch {
        backup
            .as_ref()
            .map(|b| b.fingerprint.clone())
            .unwrap_or_else(|| current_fp.clone())
    } else {
        current_fp.clone()
    };
    let source_path = if re_patch {
        backup
            .as_ref()
            .map(|b| b.primary_file())
            .unwrap_or_else(|| target.primary.clone())
    } else {
        target.primary.clone()
    };

    let mut report = Report::default();
    let mut changed = false;
    let mut new_fp = current_fp.clone();

    if already_patched && !re_patch {
        log(LogLine::ok("当前文件已经 Hook，跳过重复写入"));
        if let Some(record) = &current_record {
            report.applied = record.applied.clone();
        }
    } else if rules.is_empty() {
        log(LogLine::warn("没有适用于该启动器的规则；只启动，不修改"));
    } else {
        let bytes = fs::read(&source_path)
            .with_context(|| format!("读取启动器失败: {}", source_path.display()))?;
        log(LogLine::info(format!(
            "读取 {} 字节，开始扫描 Hook 点…",
            bytes.len()
        )));
        let (patched, r) = patch::patch_bytes(target.kind, &bytes, &rules)?;
        report = r;
        log(LogLine::info(format!(
            "命中 {} 条，跳过 {} 条",
            report.applied.len(),
            report.skipped.len()
        )));
        for a in &report.applied {
            log(LogLine::ok(format!(
                "  ↳ {}.{}{} → {}",
                short_class(&a.class),
                a.method,
                a.desc,
                a.action.describe()
            )));
        }
        for s in &report.skipped {
            log(LogLine::warn(format!("  ↳ 跳过：{s}")));
        }
        if report.applied.is_empty() {
            log(LogLine::warn(
                "没有命中任何规则；启动器可能不是受影响的版本，或已被上游改名",
            ));
        } else if opts.dry_run {
            log(LogLine::info("预览模式：不写入文件，也不启动"));
        } else {
            util::atomic_write(&target.primary, &patched)
                .with_context(|| format!("写入失败: {}", target.primary.display()))?;
            new_fp = util::sha256_hex(&patched);
            changed = true;
            let record = PatchRecord {
                schema: 1,
                source_fingerprint: source_fp.clone(),
                output_fingerprint: new_fp.clone(),
                label: target.label.clone(),
                version: target.version.clone(),
                at: util::now_secs(),
                rules_hash: Some(rules_hash.clone()),
                applied: report.applied.clone(),
            };
            let _ = record.save();
            log(LogLine::ok(format!(
                "已写入 Hook 后的启动器（新指纹 {}）",
                util::short_hash(&new_fp)
            )));
        }
    }

    state.last_path = Some(target.path.clone());
    state.last_version = Some(target.version.clone());
    state.last_label = Some(target.label.clone());
    state.last_at = Some(util::now_secs());
    if let Some(b) = &backup {
        state.last_backup_id = Some(b.id.clone());
    }
    if changed {
        state.last_original_fingerprint = Some(source_fp.clone());
        state.last_patched_fingerprint = Some(new_fp.clone());
        state.last_rules_hash = Some(rules_hash.clone());
    } else if already_patched {
        state.last_patched_fingerprint = Some(current_fp.clone());
        if state.last_original_fingerprint.is_none() {
            state.last_original_fingerprint = backup.as_ref().map(|b| b.fingerprint.clone());
        }
        state.last_rules_hash = prev_rules_hash.clone();
    }
    state.save();

    let mut pid = None;
    if opts.launch && !opts.dry_run {
        match launch(&target, log) {
            Ok(p) => pid = p,
            Err(e) => log(LogLine::error(format!("启动失败：{e}"))),
        }
    }

    Ok(HackOutcome {
        target,
        backup,
        report,
        changed,
        patched_fingerprint: changed.then_some(new_fp),
        update_detected,
        already_patched,
        pid,
    })
}

/// Keep the last two path segments so log lines fit the narrow side column.
fn short_class(class: &str) -> String {
    let sep = if class.contains('/') { '/' } else { '.' };
    let parts: Vec<&str> = class.split(sep).collect();
    if parts.len() >= 2 {
        format!(
            "{}{}{}",
            parts[parts.len() - 2],
            sep,
            parts[parts.len() - 1]
        )
    } else {
        class.to_string()
    }
}

fn launch(target: &Target, log: &mut dyn FnMut(LogLine)) -> Result<Option<u32>> {
    if !cfg!(windows) && target.kind == crate::target::TargetKind::Pe {
        log(LogLine::warn(
            "这是 Windows/.NET 启动器；当前系统不是 Windows，仍尝试启动",
        ));
    }
    let (program, args) = target.launch_command()?;
    let cwd = target
        .primary
        .parent()
        .map(Path::to_path_buf)
        .unwrap_or_else(|| Path::new(".").to_path_buf());
    log(LogLine::info(format!(
        "启动：{} {}",
        program.display(),
        args.join(" ")
    )));
    let mut child = Command::new(&program)
        .args(&args)
        .current_dir(&cwd)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .with_context(|| format!("无法启动 {}", program.display()))?;
    let pid = child.id();
    std::thread::sleep(Duration::from_millis(350));
    match child.try_wait() {
        Ok(Some(status)) => {
            log(LogLine::error(format!(
                "启动器进程已退出（{status}）。若是 HMCL，请确认 JavaFX/JRE 可用；若是 .NET，请在 Windows 上运行。"
            )));
            Ok(None)
        }
        Ok(None) => {
            log(LogLine::ok(format!("启动器已启动，PID {pid}")));
            Ok(Some(pid))
        }
        Err(e) => {
            log(LogLine::warn(format!("无法查询进程状态：{e}")));
            Ok(Some(pid))
        }
    }
}

pub fn restore(entry: &BackupEntry, log: &mut dyn FnMut(LogLine)) -> Result<()> {
    let dest = entry.restore_target();
    // Preserve whatever is currently on disk before overwriting it.
    if dest.exists() {
        if let Ok(t) = Target::detect(&dest) {
            match BackupStore::ensure(&t) {
                Ok(b) => log(LogLine::info(format!("已归档当前版本：{}", b.id))),
                Err(e) => log(LogLine::warn(format!("归档当前版本失败：{e}"))),
            }
        }
    }
    BackupStore::restore(entry)?;
    let mut state = State::load();
    state.last_path = Some(entry.original_selection.clone());
    state.last_version = Some(entry.version.clone());
    state.last_label = Some(entry.label.clone());
    state.last_backup_id = Some(entry.id.clone());
    state.last_original_fingerprint = Some(entry.fingerprint.clone());
    state.last_patched_fingerprint = None;
    state.last_at = Some(util::now_secs());
    state.save();
    log(LogLine::ok(format!(
        "已恢复 {} {} 到 {}",
        entry.label,
        entry.version,
        dest.display()
    )));
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::parse_version;

    #[test]
    fn pcl2_version_boundary() {
        assert_eq!(parse_version("2.99.0.0"), Some((2, 99, 0, 0)));
        assert_eq!(parse_version("v2.99.0.1"), Some((2, 99, 0, 1)));
        assert_eq!(parse_version("2.14.0.0"), Some((2, 14, 0, 0)));
        assert!(parse_version("2.99.0.1") > Some((2, 99, 0, 0)));
        assert!(parse_version("2.99.0.0") <= Some((2, 99, 0, 0)));
        assert_eq!(parse_version("unknown"), None);
    }
}
