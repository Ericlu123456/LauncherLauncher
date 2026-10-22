//! JAR traversal and rewrite.

use super::{Report, Rule, java};
use anyhow::Result;
use std::io::{Cursor, Read, Write};

pub fn patch_jar(bytes: &[u8], rules: &[Rule]) -> Result<(Vec<u8>, Report)> {
    let mut report = Report::default();
    let mut archive = zip::ZipArchive::new(Cursor::new(bytes))?;
    let mut entries: Vec<(String, Vec<u8>, bool, Option<u32>)> = Vec::new();
    let mut changed = false;

    for i in 0..archive.len() {
        let mut file = archive.by_index(i)?;
        let name = file.name().to_string();
        let is_dir = file.is_dir();
        let mode = file.unix_mode();
        let mut data = Vec::new();
        if !is_dir {
            file.read_to_end(&mut data)?;
        }
        let lower = name.to_ascii_lowercase();

        // A modified JAR must not keep a stale signature, or the JVM refuses it.
        if lower.starts_with("meta-inf/")
            && (lower.ends_with(".sf")
                || lower.ends_with(".rsa")
                || lower.ends_with(".dsa")
                || lower.ends_with(".ec"))
        {
            report.notes.push(format!("移除失效签名文件: {name}"));
            changed = true;
            continue;
        }

        if lower.ends_with(".class") {
            match java::patch_class(&data, rules, &mut report) {
                Ok(Some(patched)) => {
                    data = patched;
                    changed = true;
                }
                Ok(None) => {}
                Err(e) => report.notes.push(format!("跳过 {name}: {e}")),
            }
        }

        entries.push((name, data, is_dir, mode));
    }

    if !changed {
        return Ok((bytes.to_vec(), report));
    }

    let mut writer = zip::ZipWriter::new(Cursor::new(Vec::new()));
    for (name, data, is_dir, mode) in entries {
        let method = if is_dir {
            zip::CompressionMethod::Stored
        } else {
            zip::CompressionMethod::Deflated
        };
        let perms = mode.unwrap_or(if is_dir { 0o755 } else { 0o644 });
        let opts = zip::write::SimpleFileOptions::default()
            .compression_method(method)
            .unix_permissions(perms);
        writer.start_file(name, opts)?;
        if !is_dir {
            writer.write_all(&data)?;
        }
    }
    let cursor = writer.finish()?;
    Ok((cursor.into_inner(), report))
}
