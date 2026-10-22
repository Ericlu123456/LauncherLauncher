//! The hook engine.
//!
//! Two deterministic, offline backends are implemented:
//! * `jar`  — rewrites Java bytecode inside a JAR (HMCL, PCL2-CE's Java sibling builds).
//! * `pe`   — rewrites CIL method bodies inside a .NET assembly, including assemblies
//!            embedded in a .NET single-file bundle (PCL2, PCL2-CE).
//!
//! Both backends replace an entire method with a tiny constant-return body. That is
//! enough to neutralise region gates and update checks, and it avoids fragile
//! instruction-by-instruction patches that break on every launcher update.

pub mod jar;
pub mod java;
pub mod pe;
pub mod profiles;

use crate::target::TargetKind;
use crate::util;
use anyhow::{Result, bail};
use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum Action {
    ReturnTrue,
    ReturnFalse,
    ReturnVoid,
    ReturnInt(i32),
    /// PCL2 2.99: restore the real version-list loader in the download page.
    Pcl2UnlockDownload,
}

impl Action {
    pub fn short(&self) -> &'static str {
        match self {
            Self::ReturnTrue => "true",
            Self::ReturnFalse => "false",
            Self::ReturnVoid => "void",
            Self::ReturnInt(_) => "int",
            Self::Pcl2UnlockDownload => "pcl2_download",
        }
    }

    pub fn describe(&self) -> String {
        match self {
            Self::ReturnTrue => "return true".into(),
            Self::ReturnFalse => "return false".into(),
            Self::ReturnVoid => "return (void)".into(),
            Self::ReturnInt(v) => format!("return {v}"),
            Self::Pcl2UnlockDownload => "恢复 PCL2 下载页加载器".into(),
        }
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(default)]
pub struct Rule {
    pub id: String,
    /// Exact name, or a glob such as `org/jackhuang/hmcl/util/RegionUtils`.
    pub class: String,
    /// Exact name, or a glob such as `is*Update*`.
    pub method: String,
    /// Optional descriptor / signature filter. Omit to match every overload.
    pub desc: Option<String>,
    pub action: Action,
    pub note: String,
}

impl Default for Rule {
    fn default() -> Self {
        Self {
            id: String::new(),
            class: String::new(),
            method: String::new(),
            desc: None,
            action: Action::ReturnFalse,
            note: String::new(),
        }
    }
}

#[derive(Clone, Debug, Serialize, Deserialize, Default)]
#[serde(default)]
pub struct RuleSet {
    pub name: String,
    pub description: String,
    pub rules: Vec<Rule>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Applied {
    pub class: String,
    pub method: String,
    pub desc: String,
    pub action: Action,
    pub note: String,
}

#[derive(Clone, Debug, Default, Serialize)]
pub struct Report {
    pub applied: Vec<Applied>,
    pub skipped: Vec<String>,
    pub notes: Vec<String>,
}

impl Report {
    pub fn merge(&mut self, other: Report) {
        self.applied.extend(other.applied);
        self.skipped.extend(other.skipped);
        self.notes.extend(other.notes);
    }
}

pub fn name_matches(pattern: &str, text: &str) -> bool {
    if pattern.is_empty() {
        return true;
    }
    if pattern.contains('*') || pattern.contains('?') {
        util::glob_match_ci(pattern, text)
    } else {
        pattern == text
    }
}

pub fn rule_matches(rule: &Rule, class: &str, method: &str, desc: &str) -> bool {
    name_matches(&rule.class, class)
        && name_matches(&rule.method, method)
        && rule
            .desc
            .as_deref()
            .map(|d| name_matches(d, desc))
            .unwrap_or(true)
}

pub fn parse_custom_rules(text: &str) -> Result<Vec<Rule>> {
    if text.trim().is_empty() {
        return Ok(Vec::new());
    }
    let value: serde_json::Value = serde_json::from_str(text)?;
    if value.is_array() {
        Ok(serde_json::from_value(value)?)
    } else {
        let set: RuleSet = serde_json::from_value(value)?;
        Ok(set.rules)
    }
}

pub fn patch_bytes(kind: TargetKind, bytes: &[u8], rules: &[Rule]) -> Result<(Vec<u8>, Report)> {
    if rules.is_empty() {
        return Ok((bytes.to_vec(), Report::default()));
    }
    match kind {
        TargetKind::Jar => jar::patch_jar(bytes, rules),
        TargetKind::Pe => pe::patch_pe(bytes, rules),
        TargetKind::Unknown => bail!("无法识别目标类型，未执行 Hook"),
    }
}
