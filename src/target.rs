//! Target detection: what did the user point us at, and how do we identify it?

use crate::util;
use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use std::fs;
use std::io::Read;
use std::path::{Path, PathBuf};

#[derive(Clone, Copy, PartialEq, Eq, Debug, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum TargetKind {
    /// Java bytecode inside a ZIP/JAR.
    Jar,
    /// A PE image: either a plain .NET assembly or a single-file bundle containing one.
    Pe,
    Unknown,
}

impl TargetKind {
    pub fn label(self) -> &'static str {
        match self {
            Self::Jar => "Java / JAR",
            Self::Pe => ".NET / PE",
            Self::Unknown => "unknown",
        }
    }
}

#[derive(Clone, Debug)]
pub struct Target {
    /// The path the user selected (canonical when possible).
    pub path: PathBuf,
    /// For a directory selection this is the directory; for a file it is the file.
    pub root: PathBuf,
    /// The concrete launcher artifact that gets patched.
    pub primary: PathBuf,
    pub kind: TargetKind,
    pub label: String,
    pub version: String,
    pub fingerprint: String,
    pub bytes: u64,
}

impl Target {
    pub fn detect(path: &Path) -> Result<Self> {
        let path = fs::canonicalize(path).unwrap_or_else(|_| path.to_path_buf());
        if !path.exists() {
            anyhow::bail!("路径不存在: {}", path.display());
        }

        let (root, primary) = if path.is_dir() {
            let primary = find_best_artifact(&path)
                .with_context(|| format!("在目录里没有找到 .jar/.exe/.dll: {}", path.display()))?;
            (path.clone(), primary)
        } else {
            (path.clone(), path.clone())
        };

        let kind = sniff_kind(&primary)?;
        let label = label_for(&primary);
        // Prefer metadata over the file name: launchers often update in place
        // without renaming, and a manifest/assembly version is authoritative.
        let mut version = None;
        if kind == TargetKind::Jar {
            version = jar_manifest_version(&primary).ok().flatten();
        }
        if version.is_none() && kind == TargetKind::Pe {
            if let Ok(bytes) = fs::read(&primary) {
                version = crate::patch::pe::probe_assembly_version(&bytes);
            }
        }
        if version.is_none() {
            version = version_from_name(&primary);
        }
        let (fingerprint, bytes) = util::sha256_file(&primary)?;

        Ok(Self {
            path,
            root,
            primary,
            kind,
            label,
            version: version.unwrap_or_else(|| "unknown".to_string()),
            fingerprint,
            bytes,
        })
    }

    /// Relative path used inside a backup snapshot.
    pub fn primary_rel(&self) -> PathBuf {
        if self.root.is_dir() {
            self.primary
                .strip_prefix(&self.root)
                .map(Path::to_path_buf)
                .unwrap_or_else(|_| {
                    PathBuf::from(
                        self.primary
                            .file_name()
                            .unwrap_or_default()
                            .to_string_lossy()
                            .as_ref(),
                    )
                })
        } else {
            PathBuf::from(
                self.primary
                    .file_name()
                    .unwrap_or_default()
                    .to_string_lossy()
                    .as_ref(),
            )
        }
    }

    pub fn is_java(&self) -> bool {
        self.kind == TargetKind::Jar
    }

    /// The command (program + arguments) used to start this launcher.
    pub fn launch_command(&self) -> Result<(PathBuf, Vec<String>)> {
        match self.kind {
            TargetKind::Jar => {
                let java = find_java(&self.primary).unwrap_or_else(|| PathBuf::from("java"));
                Ok((
                    java,
                    vec![
                        "-jar".to_string(),
                        self.primary.to_string_lossy().to_string(),
                    ],
                ))
            }
            TargetKind::Pe => Ok((self.primary.clone(), Vec::new())),
            TargetKind::Unknown => anyhow::bail!("无法识别的启动器类型"),
        }
    }
}

fn label_for(path: &Path) -> String {
    let s = path
        .to_string_lossy()
        .to_ascii_lowercase()
        .replace('_', "-");
    if s.contains("pcl2-ce") || s.contains("pcl2ce") || s.contains("community edition") {
        "PCL2-CE".to_string()
    } else if s.contains("hmcl") {
        "HMCL".to_string()
    } else if s.contains("plain craft launcher") || s.contains("pcl") {
        "PCL2".to_string()
    } else {
        "Unknown".to_string()
    }
}

fn version_from_name(path: &Path) -> Option<String> {
    let name = path.file_name()?.to_string_lossy();
    let chars: Vec<char> = name.chars().collect();
    let mut i = 0;
    while i < chars.len() {
        if chars[i].is_ascii_digit() {
            let start = i;
            let mut dots = 0u32;
            while i < chars.len() && (chars[i].is_ascii_digit() || chars[i] == '.') {
                if chars[i] == '.' {
                    dots += 1;
                }
                i += 1;
            }
            let candidate: String = chars[start..i].iter().collect();
            let candidate = candidate.trim_matches('.').to_string();
            if dots >= 1 && candidate.len() >= 3 && candidate.len() <= 24 {
                return Some(candidate);
            }
        } else {
            i += 1;
        }
    }
    None
}

fn sniff_kind(path: &Path) -> Result<TargetKind> {
    let mut f = fs::File::open(path)?;
    let mut head = [0u8; 4];
    let n = f.read(&mut head)?;
    if n >= 4 && &head == b"PK\x03\x04" {
        return Ok(TargetKind::Jar);
    }
    if n >= 2 && &head[..2] == b"MZ" {
        return Ok(TargetKind::Pe);
    }
    // Extension fallback for unusual wrappers.
    match path
        .extension()
        .and_then(|e| e.to_str())
        .map(|e| e.to_ascii_lowercase())
        .as_deref()
    {
        Some("jar") => Ok(TargetKind::Jar),
        Some("exe") | Some("dll") => Ok(TargetKind::Pe),
        _ => Ok(TargetKind::Unknown),
    }
}

fn jar_manifest_version(path: &Path) -> Result<Option<String>> {
    let file = fs::File::open(path)?;
    let mut zip = zip::ZipArchive::new(file)?;
    let mut manifest = match zip.by_name("META-INF/MANIFEST.MF") {
        Ok(f) => f,
        Err(_) => return Ok(None),
    };
    let mut text = String::new();
    manifest.read_to_string(&mut text)?;
    for key in [
        "Implementation-Version",
        "Specification-Version",
        "Bundle-Version",
    ] {
        for line in text.lines() {
            if let Some(rest) = line.strip_prefix(&format!("{key}:")) {
                let v = rest.trim();
                if !v.is_empty() {
                    return Ok(Some(v.to_string()));
                }
            }
        }
    }
    Ok(None)
}

fn find_java(primary: &Path) -> Option<PathBuf> {
    // Prefer a JRE shipped next to the launcher, then PATH.
    let base = if primary.is_dir() {
        primary
    } else {
        primary.parent().unwrap_or(primary)
    };
    for rel in [
        "jre/bin/java",
        "jre/bin/java.exe",
        "runtime/bin/java",
        "runtime/bin/java.exe",
        "java/bin/java",
        "java/bin/java.exe",
    ] {
        let p = base.join(rel);
        if p.is_file() {
            return Some(p);
        }
    }
    None
}

fn find_best_artifact(dir: &Path) -> Option<PathBuf> {
    let mut best: Option<(i64, PathBuf)> = None;
    let mut stack = vec![(dir.to_path_buf(), 0usize)];
    while let Some((d, depth)) = stack.pop() {
        if depth > 3 {
            continue;
        }
        let entries = match fs::read_dir(&d) {
            Ok(e) => e,
            Err(_) => continue,
        };
        for entry in entries.flatten() {
            let path = entry.path();
            let name = entry.file_name().to_string_lossy().to_ascii_lowercase();
            let meta = match entry.metadata() {
                Ok(m) => m,
                Err(_) => continue,
            };
            if meta.is_dir() {
                if matches!(
                    name.as_str(),
                    "jre"
                        | "runtime"
                        | "libraries"
                        | "mods"
                        | "versions"
                        | "assets"
                        | "logs"
                        | "backup"
                        | "cache"
                        | ".git"
                        | "node_modules"
                ) {
                    continue;
                }
                stack.push((path, depth + 1));
                continue;
            }
            let ext = path
                .extension()
                .and_then(|e| e.to_str())
                .map(|e| e.to_ascii_lowercase())
                .unwrap_or_default();
            if !matches!(ext.as_str(), "jar" | "exe" | "dll") {
                continue;
            }
            let mut score = 0i64;
            if name.contains("hmcl") || name.contains("pcl") {
                score += 1000;
            }
            if depth == 0 {
                score += 300;
            }
            if ext == "jar" {
                // A folder may contain both HMCL.exe (a launch4j wrapper) and
                // HMCL.jar; the jar is the artifact that can actually be patched.
                score += 220;
            } else if ext == "exe" {
                score += 120;
            }
            if name.contains("launcher") {
                score += 60;
            }
            score += (meta.len() / (4 * 1024 * 1024)).min(80) as i64;
            if best.as_ref().map(|(s, _)| score > *s).unwrap_or(true) {
                best = Some((score, path));
            }
        }
    }
    best.map(|(_, p)| p)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn labels() {
        assert_eq!(label_for(Path::new("Plain Craft Launcher 2.exe")), "PCL2");
        assert_eq!(
            label_for(Path::new("Plain Craft Launcher Community Edition.exe")),
            "PCL2-CE"
        );
        assert_eq!(label_for(Path::new("PCL2_CE.exe")), "PCL2-CE");
        assert_eq!(label_for(Path::new("HMCL-3.7.2.jar")), "HMCL");
    }

    #[test]
    fn versions() {
        assert_eq!(
            version_from_name(Path::new("HMCL-3.7.2.jar")),
            Some("3.7.2".into())
        );
        assert_eq!(
            version_from_name(Path::new("PCL2_2.99.0.0.exe")),
            Some("2.99.0.0".into())
        );
        assert_eq!(version_from_name(Path::new("launcher.exe")), None);
    }
}
