//! Versioned launcher backups and the last-seen state.

use crate::paths;
use crate::target::Target;
use crate::util;
use anyhow::{Context, Result, bail};
use serde::{Deserialize, Serialize};
use std::fs;
use std::path::{Path, PathBuf};

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct BackupFile {
    pub path: String,
    pub sha256: String,
    pub size: u64,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct BackupEntry {
    pub schema: u32,
    pub id: String,
    pub label: String,
    pub kind: String,
    pub version: String,
    /// SHA-256 of the unmodified primary artifact.
    pub fingerprint: String,
    pub size: u64,
    pub created_at: u64,
    /// The user selected a directory (true) or a file (false).
    pub was_dir: bool,
    /// What the user originally selected.
    pub original_selection: PathBuf,
    /// Directory the primary artifact lived under.
    pub original_root: PathBuf,
    /// Primary artifact path relative to `original_root`.
    pub primary_rel: PathBuf,
    pub files: Vec<BackupFile>,
}

impl BackupEntry {
    pub fn dir(&self) -> PathBuf {
        paths::backups_dir().join(&self.id)
    }

    pub fn primary_file(&self) -> PathBuf {
        self.dir().join(&self.primary_rel)
    }

    pub fn restore_target(&self) -> PathBuf {
        if self.was_dir {
            self.original_selection.join(&self.primary_rel)
        } else {
            self.original_selection.clone()
        }
    }
}

#[derive(Clone, Debug, Serialize, Deserialize, Default)]
#[serde(default)]
pub struct State {
    pub last_original_fingerprint: Option<String>,
    pub last_patched_fingerprint: Option<String>,
    pub last_backup_id: Option<String>,
    pub last_path: Option<PathBuf>,
    pub last_version: Option<String>,
    pub last_label: Option<String>,
    pub last_at: Option<u64>,
    pub last_rules_hash: Option<String>,
}

impl State {
    pub fn load() -> Self {
        fs::read_to_string(paths::state_file())
            .ok()
            .and_then(|s| serde_json::from_str(&s).ok())
            .unwrap_or_default()
    }

    pub fn save(&self) {
        if let Ok(json) = serde_json::to_vec_pretty(self) {
            let _ = util::atomic_write(&paths::state_file(), &json);
        }
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct PatchRecord {
    pub schema: u32,
    pub source_fingerprint: String,
    pub output_fingerprint: String,
    pub label: String,
    pub version: String,
    pub at: u64,
    #[serde(default)]
    pub rules_hash: Option<String>,
    pub applied: Vec<crate::patch::Applied>,
}

impl PatchRecord {
    pub fn save(&self) -> Result<()> {
        let path = paths::patched_dir().join(format!("{}.json", self.output_fingerprint));
        let json = serde_json::to_vec_pretty(self)?;
        util::atomic_write(&path, &json)?;
        Ok(())
    }

    pub fn load_for_output(fingerprint: &str) -> Option<Self> {
        let path = paths::patched_dir().join(format!("{fingerprint}.json"));
        fs::read_to_string(path)
            .ok()
            .and_then(|s| serde_json::from_str(&s).ok())
    }
}

pub struct BackupStore;

impl BackupStore {
    /// Ensure a snapshot of `target`'s current primary artifact exists.
    pub fn ensure(target: &Target) -> Result<BackupEntry> {
        paths::ensure_dirs()?;
        let short = util::short_hash(&target.fingerprint);
        let base_id = format!(
            "{}-{}-{}",
            util::sanitize_component(&target.label),
            util::sanitize_component(&target.version),
            short
        );

        let mut id = base_id.clone();
        let mut n = 2u32;
        loop {
            let dir = paths::backups_dir().join(&id);
            if !dir.exists() {
                break;
            }
            if let Some(existing) = read_manifest(&dir) {
                if existing.fingerprint == target.fingerprint {
                    return Ok(existing);
                }
            }
            id = format!("{base_id}-{n}");
            n += 1;
            if n > 100 {
                bail!("备份目录冲突过多: {base_id}");
            }
        }

        let tmp = paths::backups_dir().join(format!(".tmp-{id}-{}", std::process::id()));
        if tmp.exists() {
            fs::remove_dir_all(&tmp)?;
        }
        fs::create_dir_all(&tmp)?;
        let rel = target.primary_rel();
        let dest = tmp.join(&rel);
        if let Some(parent) = dest.parent() {
            fs::create_dir_all(parent)?;
        }
        fs::copy(&target.primary, &dest)
            .with_context(|| format!("复制启动器失败: {}", target.primary.display()))?;
        let (sha, size) = util::sha256_file(&dest)?;
        if sha != target.fingerprint {
            let _ = fs::remove_dir_all(&tmp);
            bail!("启动器在备份过程中发生变化，请重试");
        }

        let manifest = BackupEntry {
            schema: 1,
            id: id.clone(),
            label: target.label.clone(),
            kind: target.kind.label().to_string(),
            version: target.version.clone(),
            fingerprint: target.fingerprint.clone(),
            size,
            created_at: util::now_secs(),
            was_dir: target.root.is_dir(),
            original_selection: target.path.clone(),
            original_root: target.root.clone(),
            primary_rel: rel.clone(),
            files: vec![BackupFile {
                path: rel.to_string_lossy().to_string(),
                sha256: sha,
                size,
            }],
        };
        let json = serde_json::to_vec_pretty(&manifest)?;
        fs::write(tmp.join("manifest.json"), json)?;

        let final_dir = paths::backups_dir().join(&id);
        match fs::rename(&tmp, &final_dir) {
            Ok(()) => {}
            Err(_) if final_dir.exists() => {
                let _ = fs::remove_dir_all(&tmp);
                if let Some(existing) = read_manifest(&final_dir) {
                    if existing.fingerprint == target.fingerprint {
                        return Ok(existing);
                    }
                }
                bail!("备份目录已存在但内容不匹配: {}", final_dir.display());
            }
            Err(e) => {
                let _ = fs::remove_dir_all(&tmp);
                return Err(e.into());
            }
        }
        Ok(manifest)
    }

    pub fn list() -> Vec<BackupEntry> {
        let mut out = Vec::new();
        let Ok(entries) = fs::read_dir(paths::backups_dir()) else {
            return out;
        };
        for entry in entries.flatten() {
            if !entry.file_type().map(|t| t.is_dir()).unwrap_or(false) {
                continue;
            }
            if let Some(m) = read_manifest(&entry.path()) {
                out.push(m);
            }
        }
        out.sort_by(|a, b| b.created_at.cmp(&a.created_at));
        out
    }

    pub fn find_by_fingerprint(fingerprint: &str) -> Option<BackupEntry> {
        Self::list()
            .into_iter()
            .find(|b| b.fingerprint == fingerprint)
    }

    pub fn latest() -> Option<BackupEntry> {
        Self::list().into_iter().next()
    }

    /// Copy a snapshot back over the launcher. The current file is not backed
    /// up here; callers should call `ensure` first if that matters.
    pub fn restore(entry: &BackupEntry) -> Result<()> {
        let src = entry.primary_file();
        if !src.is_file() {
            bail!("备份文件缺失: {}", src.display());
        }
        let bytes = fs::read(&src)?;
        let (sha, _) = (util::sha256_hex(&bytes), bytes.len());
        if let Some(expected) = entry.files.first() {
            if expected.sha256 != sha {
                bail!("备份校验失败：文件已被修改");
            }
        }
        let dest = entry.restore_target();
        util::atomic_write(&dest, &bytes)
            .with_context(|| format!("恢复失败: {}", dest.display()))?;
        Ok(())
    }
}

fn read_manifest(dir: &Path) -> Option<BackupEntry> {
    let s = fs::read_to_string(dir.join("manifest.json")).ok()?;
    serde_json::from_str(&s).ok()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::target::Target;

    #[test]
    fn backup_versioning_and_restore() {
        let home = std::env::temp_dir().join(format!("ll-home-{}", std::process::id()));
        let _ = fs::remove_dir_all(&home);
        fs::create_dir_all(&home).unwrap();
        // SAFETY: this test is the only one touching the process environment
        // in this binary and runs on a single test thread.
        unsafe { std::env::set_var("LL_HOME", &home) };

        let launcher = home.join("HMCL-3.7.2.jar");
        fs::write(&launcher, b"PK\x03\x04original-one").unwrap();
        let t1 = Target::detect(&launcher).unwrap();
        let b1 = BackupStore::ensure(&t1).unwrap();
        assert_eq!(b1.fingerprint, t1.fingerprint);
        assert!(b1.primary_file().is_file());

        fs::write(&launcher, b"PK\x03\x04updated-two").unwrap();
        let t2 = Target::detect(&launcher).unwrap();
        assert_ne!(t1.fingerprint, t2.fingerprint);
        let b2 = BackupStore::ensure(&t2).unwrap();
        assert_ne!(b1.id, b2.id);
        assert_eq!(BackupStore::list().len(), 2);

        fs::write(&launcher, b"PK\x03\x04patched-three").unwrap();
        BackupStore::restore(&b2).unwrap();
        assert_eq!(fs::read(&launcher).unwrap(), b"PK\x03\x04updated-two");
        BackupStore::restore(&b1).unwrap();
        assert_eq!(fs::read(&launcher).unwrap(), b"PK\x03\x04original-one");

        let _ = fs::remove_dir_all(&home);
        unsafe { std::env::remove_var("LL_HOME") };
    }
}
