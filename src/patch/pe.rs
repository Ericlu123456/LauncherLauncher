//! A narrow .NET PE/CIL patcher.
//!
//! It parses just enough of ECMA-335 to find a `TypeDef` -> `MethodDef` and to
//! replace the method body with `ldc.i4.N; ret`. That covers boolean/void gates
//! such as `RegionAssessment.get_IsRestricted` and `UpdateChecker.isOutdated`.
//!
//! The same code also walks a .NET single-file bundle: embedded managed
//! assemblies are ordinary PE images, so we locate them by scanning for `MZ`
//! and patch them in place. The bundle entries keep their size, which means the
//! containing single-file layout stays valid.

use super::{Action, Applied, Report, Rule, rule_matches};
use anyhow::{Context, Result, bail};
use std::collections::{HashMap, HashSet};

#[derive(Clone, Debug)]
struct Section {
    va: u32,
    vsize: u32,
    raw_off: u32,
    raw_size: u32,
}

#[derive(Clone, Debug)]
struct PeImage {
    base: usize,
    sections: Vec<Section>,
    header_size: u32,
    meta_off: Option<usize>,
}

impl PeImage {
    fn parse(bytes: &[u8], base: usize) -> Result<Self> {
        if read(bytes, base, 2)? != b"MZ".as_slice() {
            bail!("not MZ");
        }
        let lfanew = u32_at(bytes, base + 0x3C)? as usize;
        if lfanew < 0x40 || lfanew > 0x1000 {
            bail!("bad e_lfanew");
        }
        let pe = base
            .checked_add(lfanew)
            .context("PE header offset overflow")?;
        if read(bytes, pe, 4)? != b"PE\0\0".as_slice() {
            bail!("bad PE signature");
        }
        let num_sections = u16_at(bytes, pe + 6)? as usize;
        let opt_size = u16_at(bytes, pe + 20)? as usize;
        let opt = pe + 24;
        if opt + opt_size > bytes.len() {
            bail!("optional header out of bounds");
        }
        let magic = u16_at(bytes, opt)?;
        let (dirs_off, dd_count) = match magic {
            0x10b => (opt + 96, u32_at(bytes, opt + 92)?),
            0x20b => (opt + 112, u32_at(bytes, opt + 108)?),
            _ => bail!("unknown optional header magic {magic:#x}"),
        };
        let header_size = u32_at(bytes, opt + 60)?;
        let cli_rva = if dd_count > 14 {
            u32_at(bytes, dirs_off + 14 * 8)?
        } else {
            0
        };

        let sec_off = opt + opt_size;
        let mut sections = Vec::with_capacity(num_sections);
        for i in 0..num_sections {
            let off = sec_off + i * 40;
            if off + 40 > bytes.len() {
                bail!("section table out of bounds");
            }
            sections.push(Section {
                vsize: u32_at(bytes, off + 8)?,
                va: u32_at(bytes, off + 12)?,
                raw_size: u32_at(bytes, off + 16)?,
                raw_off: u32_at(bytes, off + 20)?,
            });
        }

        let mut img = Self {
            base,
            sections,
            header_size,
            meta_off: None,
        };
        if cli_rva != 0 {
            if let Some(cli_off) = img.rva_to_off(cli_rva) {
                if cli_off + 16 <= bytes.len() {
                    let meta_rva = u32_at(bytes, cli_off + 8)?;
                    if meta_rva != 0 {
                        if let Some(meta_off) = img.rva_to_off(meta_rva) {
                            if read(bytes, meta_off, 4)? == b"BSJB" {
                                img.meta_off = Some(meta_off);
                            }
                        }
                    }
                }
            }
        }
        Ok(img)
    }

    fn rva_to_off(&self, rva: u32) -> Option<usize> {
        if rva < self.header_size {
            return Some(self.base + rva as usize);
        }
        for s in &self.sections {
            let size = s.vsize.max(s.raw_size);
            if rva >= s.va && rva < s.va.saturating_add(size) {
                let delta = rva - s.va;
                if delta >= s.raw_size && s.raw_size != 0 {
                    return None;
                }
                return Some(self.base + s.raw_off as usize + delta as usize);
            }
        }
        None
    }
}

#[allow(dead_code)]
#[derive(Clone, Copy, Debug)]
enum Col {
    U8,
    U16,
    U32,
    Str,
    Guid,
    Blob,
    Rid(u8),
    Coded(u8, &'static [u8]),
}

const TYPE_DEF_OR_REF: &[u8] = &[0x02, 0x01, 0x1B];
const HAS_CONSTANT: &[u8] = &[0x04, 0x08, 0x17];
const HAS_CUSTOM_ATTRIBUTE: &[u8] = &[
    0x06, 0x04, 0x01, 0x02, 0x08, 0x09, 0x0A, 0x00, 0x0E, 0x17, 0x14, 0x11, 0x1A, 0x1B, 0x20, 0x23,
    0x26, 0x27, 0x28, 0x2A, 0x2B, 0x2C,
];
const HAS_FIELD_MARSHAL: &[u8] = &[0x04, 0x08];
const HAS_DECL_SECURITY: &[u8] = &[0x02, 0x06, 0x20];
const MEMBER_REF_PARENT: &[u8] = &[0x02, 0x01, 0x1A, 0x06, 0x1B];
const HAS_SEMANTICS: &[u8] = &[0x14, 0x17];
const METHOD_DEF_OR_REF: &[u8] = &[0x06, 0x0A];
const MEMBER_FORWARDED: &[u8] = &[0x04, 0x06];
const IMPLEMENTATION: &[u8] = &[0x26, 0x23, 0x27];
const CUSTOM_ATTRIBUTE_TYPE: &[u8] = &[0x00, 0x00, 0x06, 0x0A, 0x00];
const RESOLUTION_SCOPE: &[u8] = &[0x00, 0x1A, 0x23, 0x01];
const TYPE_OR_METHOD_DEF: &[u8] = &[0x02, 0x06];

fn columns(table: u8) -> &'static [Col] {
    use Col::*;
    match table {
        0x00 => &[U16, Str, Guid, Guid, Guid],
        0x01 => &[Coded(2, RESOLUTION_SCOPE), Str, Str],
        0x02 => &[
            U32,
            Str,
            Str,
            Coded(2, TYPE_DEF_OR_REF),
            Rid(0x04),
            Rid(0x06),
        ],
        0x03 => &[Rid(0x04)],
        0x04 => &[U16, Str, Blob],
        0x05 => &[Rid(0x06)],
        0x06 => &[U32, U16, U16, Str, Blob, Rid(0x08)],
        0x07 => &[Rid(0x08)],
        0x08 => &[U16, U16, Str],
        0x09 => &[Rid(0x02), Coded(2, TYPE_DEF_OR_REF)],
        0x0A => &[Coded(3, MEMBER_REF_PARENT), Str, Blob],
        0x0B => &[U16, Coded(2, HAS_CONSTANT), Blob],
        0x0C => &[
            Coded(5, HAS_CUSTOM_ATTRIBUTE),
            Coded(3, CUSTOM_ATTRIBUTE_TYPE),
            Blob,
        ],
        0x0D => &[Coded(1, HAS_FIELD_MARSHAL), Blob],
        0x0E => &[U16, Coded(2, HAS_DECL_SECURITY), Blob],
        0x0F => &[U16, U32, Rid(0x02)],
        0x10 => &[U32, Rid(0x04)],
        0x11 => &[Blob],
        0x12 => &[Rid(0x02), Rid(0x14)],
        0x13 => &[Rid(0x14)],
        0x14 => &[U16, Str, Coded(2, TYPE_DEF_OR_REF)],
        0x15 => &[Rid(0x02), Rid(0x17)],
        0x16 => &[Rid(0x17)],
        0x17 => &[U16, Str, Blob],
        0x18 => &[U16, Rid(0x06), Coded(1, HAS_SEMANTICS)],
        0x19 => &[
            Rid(0x02),
            Coded(1, METHOD_DEF_OR_REF),
            Coded(1, METHOD_DEF_OR_REF),
        ],
        0x1A => &[Str],
        0x1B => &[Blob],
        0x1C => &[U16, Coded(1, MEMBER_FORWARDED), Str, Rid(0x1A)],
        0x1D => &[U32, Rid(0x04)],
        0x1E => &[U32, U32],
        0x1F => &[U32],
        0x20 => &[U32, U16, U16, U16, U16, U32, Blob, Str, Str],
        0x21 => &[U32],
        0x22 => &[U32, U32, U32],
        0x23 => &[U16, U16, U16, U16, U32, Blob, Str, Str, Blob],
        0x24 => &[U32, Rid(0x23)],
        0x25 => &[U32, U32, U32, Rid(0x23)],
        0x26 => &[U32, Str, Blob],
        0x27 => &[U32, U32, Str, Str, Coded(2, IMPLEMENTATION)],
        0x28 => &[U32, U32, Str, Coded(2, IMPLEMENTATION)],
        0x29 => &[Rid(0x02), Rid(0x02)],
        0x2A => &[U16, U16, Coded(1, TYPE_OR_METHOD_DEF), Str],
        0x2B => &[Coded(1, METHOD_DEF_OR_REF), Blob],
        0x2C => &[Rid(0x2A), Coded(2, TYPE_DEF_OR_REF)],
        _ => &[],
    }
}

struct Metadata {
    strings_off: usize,
    strings_size: usize,
    blob_off: usize,
    blob_size: usize,
    us_off: usize,
    us_size: usize,
    heap_sizes: u8,
    counts: [u32; 64],
    table_offsets: [usize; 64],
    row_sizes: [usize; 64],
}

impl Metadata {
    fn parse(bytes: &[u8], meta_off: usize) -> Result<Self> {
        if read(bytes, meta_off, 4)? != b"BSJB" {
            bail!("bad metadata signature");
        }
        let ver_len = u32_at(bytes, meta_off + 12)? as usize;
        let ver_end = meta_off + 16 + ver_len;
        let mut p = (ver_end + 3) & !3;
        let _flags = u16_at(bytes, p)?;
        let streams = u16_at(bytes, p + 2)? as usize;
        p += 4;

        let mut tables = None;
        let mut strings = None;
        let mut blob = None;
        let mut us = None;
        for _ in 0..streams {
            let off = u32_at(bytes, p)? as usize;
            let size = u32_at(bytes, p + 4)? as usize;
            p += 8;
            let start = p;
            while p < bytes.len() && bytes[p] != 0 {
                p += 1;
            }
            let name = String::from_utf8_lossy(&bytes[start..p]).into_owned();
            p = (p + 1 + 3) & !3;
            let abs = meta_off + off;
            match name.as_str() {
                "#~" | "#-" => tables = Some((abs, size)),
                "#Strings" => strings = Some((abs, size)),
                "#Blob" => blob = Some((abs, size)),
                "#US" => us = Some((abs, size)),
                "#GUID" => {}
                _ => {}
            }
        }
        let (tables_off, tables_size) = tables.context("#~ stream missing")?;
        let (strings_off, strings_size) = strings.context("#Strings stream missing")?;
        let (blob_off, blob_size) = blob.context("#Blob stream missing")?;
        let (us_off, us_size) = us.unwrap_or((0, 0));
        if tables_off + 24 > bytes.len() || tables_off + tables_size > bytes.len() {
            bail!("#~ stream out of bounds");
        }
        let heap_sizes = bytes[tables_off + 6];
        let valid = u64::from_le_bytes(
            bytes[tables_off + 8..tables_off + 16]
                .try_into()
                .context("valid mask")?,
        );

        let mut counts = [0u32; 64];
        let mut p = tables_off + 24;
        for (t, count) in counts.iter_mut().enumerate() {
            if valid & (1u64 << t) != 0 {
                *count = u32_at(bytes, p)?;
                p += 4;
            }
        }

        let mut row_sizes = [0usize; 64];
        for t in 0..64usize {
            row_sizes[t] = row_size(t as u8, &counts, heap_sizes);
        }
        let mut table_offsets = [0usize; 64];
        let mut off = p;
        for t in 0..64usize {
            table_offsets[t] = off;
            off = off.saturating_add(row_sizes[t].saturating_mul(counts[t] as usize));
        }

        Ok(Self {
            strings_off,
            strings_size,
            blob_off,
            blob_size,
            us_off,
            us_size,
            heap_sizes,
            counts,
            table_offsets,
            row_sizes,
        })
    }

    fn col_size(&self, col: Col) -> usize {
        match col {
            Col::U8 => 1,
            Col::U16 => 2,
            Col::U32 => 4,
            Col::Str => {
                if self.heap_sizes & 1 != 0 {
                    4
                } else {
                    2
                }
            }
            Col::Guid => {
                if self.heap_sizes & 2 != 0 {
                    4
                } else {
                    2
                }
            }
            Col::Blob => {
                if self.heap_sizes & 4 != 0 {
                    4
                } else {
                    2
                }
            }
            Col::Rid(t) => {
                if self.counts[t as usize] < 0x1_0000 {
                    2
                } else {
                    4
                }
            }
            Col::Coded(bits, tables) => {
                let max = tables
                    .iter()
                    .map(|t| self.counts[*t as usize])
                    .max()
                    .unwrap_or(0);
                if max < (1u32 << (16 - bits)) { 2 } else { 4 }
            }
        }
    }

    fn read_col(&self, bytes: &[u8], table: u8, row: u32, col: usize) -> Result<u32> {
        if row == 0 || row > self.counts[table as usize] {
            bail!("table {table:#x} row {row} out of range");
        }
        let mut off = self.table_offsets[table as usize]
            + (row as usize - 1) * self.row_sizes[table as usize];
        for (i, c) in columns(table).iter().enumerate() {
            let size = self.col_size(*c);
            if i == col {
                return read_uint(bytes, off, size);
            }
            off += size;
        }
        bail!("column {col} out of range for table {table:#x}")
    }

    fn string(&self, bytes: &[u8], heap_off: u32) -> Result<String> {
        let start = self.strings_off + heap_off as usize;
        if heap_off as usize >= self.strings_size || start >= bytes.len() {
            bail!("string heap offset out of range");
        }
        let end = start
            .saturating_add(self.strings_size - heap_off as usize)
            .min(bytes.len());
        let slice = &bytes[start..end];
        let len = slice.iter().position(|b| *b == 0).unwrap_or(slice.len());
        Ok(String::from_utf8_lossy(&slice[..len]).into_owned())
    }

    /// Look up a FieldDef row by its simple name (the PCL2 loader field is
    /// unique in its assembly).
    fn field_by_name(&self, bytes: &[u8], name: &str) -> Result<Option<u32>> {
        for rid in 1..=self.counts[0x04] {
            let name_idx = self.read_col(bytes, 0x04, rid, 1)?;
            if self.string(bytes, name_idx)? == name {
                return Ok(Some(rid));
            }
        }
        Ok(None)
    }

    /// Resolve a `#US` user-string token (0x70xxxxxx) to UTF-16 text.
    fn user_string(&self, bytes: &[u8], token: u32) -> Result<String> {
        if token >> 24 != 0x70 {
            bail!("not a user-string token");
        }
        let off = (token & 0x00FF_FFFF) as usize;
        if self.us_size == 0 || off >= self.us_size || self.us_off + off >= bytes.len() {
            bail!("user-string offset out of range");
        }
        let mut p = self.us_off + off;
        let len = read_compressed(bytes, &mut p)? as usize;
        let end = p.checked_add(len).context("user-string length overflow")?;
        if end > bytes.len() {
            bail!("user-string out of bounds");
        }
        let units: Vec<u16> = bytes[p..end]
            .chunks_exact(2)
            .map(|c| u16::from_le_bytes([c[0], c[1]]))
            .collect();
        Ok(String::from_utf16_lossy(&units))
    }

    /// True for a `.ctor` whose declaring type is a generic TypeSpec
    /// (`LoaderTask`2`, `Action`1`, ...). PCL2's loader ctor is the second
    /// such ctor after the "关闭下载" string.
    fn memberref_generic_ctor(&self, bytes: &[u8], token: u32) -> bool {
        if token >> 24 != 0x0A {
            return false;
        }
        let rid = token & 0x00FF_FFFF;
        let name = self
            .read_col(bytes, 0x0A, rid, 1)
            .ok()
            .and_then(|i| self.string(bytes, i).ok())
            .unwrap_or_default();
        if name != ".ctor" {
            return false;
        }
        let parent = self.read_col(bytes, 0x0A, rid, 0).unwrap_or(0);
        (parent & 0x7) == 4 // TypeSpec
    }

    /// Resolve a MemberRef's declaring type name (TypeRef or TypeDef).
    fn memberref_parent_name(&self, bytes: &[u8], token: u32) -> Option<String> {
        if token >> 24 != 0x0A {
            return None;
        }
        let rid = token & 0x00FF_FFFF;
        let parent = self.read_col(bytes, 0x0A, rid, 0).ok()?;
        let tag = parent & 0x7;
        let idx = parent >> 3;
        let table = match tag {
            0 => 0x02, // TypeDef
            1 => 0x01, // TypeRef
            _ => return None,
        };
        let name_idx = self.read_col(bytes, table, idx, 1).ok()?;
        let ns_idx = self.read_col(bytes, table, idx, 2).ok()?;
        let name = self.string(bytes, name_idx).ok()?;
        let ns = self.string(bytes, ns_idx).ok()?;
        Some(if ns.is_empty() {
            name
        } else {
            format!("{ns}.{name}")
        })
    }

    fn blob<'a>(&self, bytes: &'a [u8], heap_off: u32) -> Result<&'a [u8]> {
        let start = self.blob_off + heap_off as usize;
        if heap_off as usize >= self.blob_size || start >= bytes.len() {
            bail!("blob heap offset out of range");
        }
        let mut p = start;
        let len = read_compressed(bytes, &mut p)? as usize;
        let end = p.checked_add(len).context("blob length overflow")?;
        if end > bytes.len() {
            bail!("blob out of bounds");
        }
        Ok(&bytes[p..end])
    }

    fn assembly_name_version(&self, bytes: &[u8]) -> Result<(String, String)> {
        if self.counts[0x20] == 0 {
            bail!("no Assembly table");
        }
        let major = self.read_col(bytes, 0x20, 1, 1)?;
        let minor = self.read_col(bytes, 0x20, 1, 2)?;
        let build = self.read_col(bytes, 0x20, 1, 3)?;
        let rev = self.read_col(bytes, 0x20, 1, 4)?;
        let name_idx = self.read_col(bytes, 0x20, 1, 7)?;
        let name = self.string(bytes, name_idx).unwrap_or_default();
        Ok((name, format!("{major}.{minor}.{build}.{rev}")))
    }
}

fn row_size(table: u8, counts: &[u32; 64], heap_sizes: u8) -> usize {
    let mut n = 0usize;
    for c in columns(table) {
        n += match c {
            Col::U8 => 1,
            Col::U16 => 2,
            Col::U32 => 4,
            Col::Str => {
                if heap_sizes & 1 != 0 {
                    4
                } else {
                    2
                }
            }
            Col::Guid => {
                if heap_sizes & 2 != 0 {
                    4
                } else {
                    2
                }
            }
            Col::Blob => {
                if heap_sizes & 4 != 0 {
                    4
                } else {
                    2
                }
            }
            Col::Rid(t) => {
                if counts[*t as usize] < 0x1_0000 {
                    2
                } else {
                    4
                }
            }
            Col::Coded(bits, tables) => {
                let max = tables
                    .iter()
                    .map(|t| counts[*t as usize])
                    .max()
                    .unwrap_or(0);
                if max < (1u32 << (16 - bits)) { 2 } else { 4 }
            }
        };
    }
    n
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Ret {
    Void,
    Bool,
    I4,
    Other,
}

impl Ret {
    fn tag(self) -> &'static str {
        match self {
            Self::Void => "V",
            Self::Bool => "Z",
            Self::I4 => "I",
            Self::Other => "?",
        }
    }
}

struct MethodTarget {
    class: String,
    method: String,
    desc: String,
    rva: u32,
    action: Action,
    note: String,
}

fn collect_targets(
    bytes: &[u8],
    meta: &Metadata,
    pe: &PeImage,
    rules: &[Rule],
) -> Vec<MethodTarget> {
    let mut out = Vec::new();
    let type_count = meta.counts[0x02];
    if type_count == 0 {
        return out;
    }

    // Nested type -> enclosing type, so `Outer+Inner` can be matched.
    let mut nested_parent: HashMap<u32, u32> = HashMap::new();
    if meta.counts[0x29] > 0 {
        for row in 1..=meta.counts[0x29] {
            if let (Ok(nested), Ok(parent)) = (
                meta.read_col(bytes, 0x29, row, 0),
                meta.read_col(bytes, 0x29, row, 1),
            ) {
                nested_parent.insert(nested, parent);
            }
        }
    }

    let mut name_cache: HashMap<u32, String> = HashMap::new();
    for trow in 1..=type_count {
        let name_idx = match meta.read_col(bytes, 0x02, trow, 1) {
            Ok(v) => v,
            Err(_) => continue,
        };
        let ns_idx = meta.read_col(bytes, 0x02, trow, 2).unwrap_or(0);
        let simple = meta.string(bytes, name_idx).unwrap_or_default();
        let ns = meta.string(bytes, ns_idx).unwrap_or_default();
        let full = type_full_name(
            trow,
            &simple,
            &ns,
            &nested_parent,
            meta,
            bytes,
            &mut name_cache,
        );

        let class_rules: Vec<&Rule> = rules
            .iter()
            .filter(|r| super::name_matches(&r.class, &full))
            .collect();
        if class_rules.is_empty() {
            continue;
        }

        let start = meta.read_col(bytes, 0x02, trow, 5).unwrap_or(0);
        let end = if trow < type_count {
            meta.read_col(bytes, 0x02, trow + 1, 5).unwrap_or(start)
        } else {
            meta.counts[0x06] + 1
        };
        if start == 0 || end <= start {
            continue;
        }
        for mrow in start..end.min(meta.counts[0x06] + 1) {
            let name_idx = match meta.read_col(bytes, 0x06, mrow, 3) {
                Ok(v) => v,
                Err(_) => continue,
            };
            let method = meta.string(bytes, name_idx).unwrap_or_default();
            if method == ".ctor" || method == ".cctor" {
                continue;
            }
            let sig_idx = meta.read_col(bytes, 0x06, mrow, 4).unwrap_or(0);
            let sig = meta.blob(bytes, sig_idx).unwrap_or(&[]);
            let ret = return_kind(sig);
            let desc = ret.tag();
            let rva = meta.read_col(bytes, 0x06, mrow, 0).unwrap_or(0);
            if rva == 0 {
                continue;
            }
            if pe.rva_to_off(rva).is_none() {
                continue;
            }
            if let Some(rule) = class_rules
                .iter()
                .find(|r| rule_matches(r, &full, &method, desc))
            {
                if !action_compatible(&rule.action, ret) {
                    continue;
                }
                out.push(MethodTarget {
                    class: full.clone(),
                    method,
                    desc: desc.to_string(),
                    rva,
                    action: rule.action.clone(),
                    note: rule.note.clone(),
                });
            }
        }
    }
    out
}

fn type_full_name(
    rid: u32,
    simple: &str,
    ns: &str,
    nested_parent: &HashMap<u32, u32>,
    meta: &Metadata,
    bytes: &[u8],
    cache: &mut HashMap<u32, String>,
) -> String {
    if let Some(v) = cache.get(&rid) {
        return v.clone();
    }
    if let Some(parent) = nested_parent.get(&rid) {
        let p_simple = meta
            .read_col(bytes, 0x02, *parent, 1)
            .ok()
            .and_then(|i| meta.string(bytes, i).ok())
            .unwrap_or_default();
        let p_ns = meta
            .read_col(bytes, 0x02, *parent, 2)
            .ok()
            .and_then(|i| meta.string(bytes, i).ok())
            .unwrap_or_default();
        let parent_name =
            type_full_name(*parent, &p_simple, &p_ns, nested_parent, meta, bytes, cache);
        let full = format!("{parent_name}+{simple}");
        cache.insert(rid, full.clone());
        full
    } else {
        let full = if ns.is_empty() {
            simple.to_string()
        } else {
            format!("{ns}.{simple}")
        };
        cache.insert(rid, full.clone());
        full
    }
}

fn action_compatible(action: &Action, ret: Ret) -> bool {
    match action {
        Action::ReturnTrue | Action::ReturnFalse => ret == Ret::Bool,
        Action::ReturnVoid => ret == Ret::Void,
        Action::ReturnInt(_) => ret == Ret::I4,
        Action::Pcl2UnlockDownload => ret == Ret::Void,
    }
}

fn return_kind(sig: &[u8]) -> Ret {
    if sig.is_empty() {
        return Ret::Other;
    }
    let mut p = 0usize;
    let cc = sig[0];
    p += 1;
    if cc & 0x10 != 0 {
        let _ = read_compressed(sig, &mut p);
    }
    let _ = read_compressed(sig, &mut p); // parameter count
    loop {
        let Some(e) = sig.get(p).copied() else {
            return Ret::Other;
        };
        p += 1;
        match e {
            0x1F | 0x20 => {
                // cmod_reqd / cmod_opt: a TypeDefOrRef token follows.
                let _ = read_compressed(sig, &mut p);
                continue;
            }
            0x45 => continue, // pinned
            0x01 => return Ret::Void,
            0x02 => return Ret::Bool,
            0x08 => return Ret::I4,
            _ => return Ret::Other,
        }
    }
}

fn method_code(
    bytes: &[u8],
    code_off: usize,
) -> Option<(usize, usize, Option<usize>, Option<usize>)> {
    let b0 = *bytes.get(code_off)?;
    if b0 & 3 == 2 {
        let size = (b0 >> 2) as usize;
        if size == 0 {
            return None;
        }
        Some((code_off + 1, size, None, None))
    } else if b0 & 3 == 3 {
        if code_off + 12 > bytes.len() {
            return None;
        }
        let flags = u16::from_le_bytes([bytes[code_off], bytes[code_off + 1]]);
        let header_size = ((flags >> 12) as usize) * 4;
        if header_size < 12 || code_off + header_size > bytes.len() {
            return None;
        }
        let code_size = u32::from_le_bytes([
            bytes[code_off + 4],
            bytes[code_off + 5],
            bytes[code_off + 6],
            bytes[code_off + 7],
        ]) as usize;
        if code_size == 0 {
            return None;
        }
        Some((
            code_off + header_size,
            code_size,
            Some(code_off),
            Some(code_off + 2),
        ))
    } else {
        None
    }
}

/// PCL2 2.99 replaced the download page's real `DlClientListLoader` with a
/// throwing `LoaderTask("关闭下载", ...)`. The IL around that construction is:
///
///   ldstr "关闭下载"; delegate cache; newobj LoaderTask`2::.ctor
///
/// We overwrite that whole span with `ldsfld ModDownload::DlClientListLoader`
/// and nops. The rest of the method (the sixth argument and the
/// `PageLoaderInit` call) stays intact, so the page binds to the real loader.
fn patch_pcl2_loader(
    out: &mut [u8],
    meta: &Metadata,
    body_off: usize,
    body_size: usize,
) -> Result<()> {
    let body_end = body_off
        .checked_add(body_size)
        .context("IL body end overflow")?;
    if body_end > out.len() {
        bail!("IL body out of bounds");
    }

    let mut start = None;
    let mut p = body_off;
    while p + 5 <= body_end {
        if out[p] == 0x72 {
            let token = u32::from_le_bytes([out[p + 1], out[p + 2], out[p + 3], out[p + 4]]);
            if meta.user_string(out, token).ok().as_deref() == Some("关闭下载") {
                start = Some(p);
                break;
            }
        }
        p += 1;
    }
    let start = start.context("未找到 PCL2 的“关闭下载”分支")?;

    let mut end = None;
    let mut generic_ctors = 0;
    let mut p = start + 5;
    while p + 5 <= body_end {
        if out[p] == 0x73 {
            let token = u32::from_le_bytes([out[p + 1], out[p + 2], out[p + 3], out[p + 4]]);
            if meta.memberref_generic_ctor(out, token) {
                generic_ctors += 1;
                if generic_ctors == 2 {
                    end = Some(p + 5);
                    break;
                }
            } else if meta
                .memberref_parent_name(out, token)
                .map(|n| n.contains("LoaderTask"))
                .unwrap_or(false)
            {
                end = Some(p + 5);
                break;
            }
        }
        p += 1;
    }
    let end = end.context("未找到 LoaderTask 构造调用")?;
    if end <= start + 5 {
        bail!("下载分支长度异常");
    }

    let rid = meta
        .field_by_name(out, "DlClientListLoader")?
        .context("未找到 ModDownload.DlClientListLoader 字段")?;
    let token = 0x0400_0000 | rid;
    out[start] = 0x7E; // ldsfld
    out[start + 1..start + 5].copy_from_slice(&token.to_le_bytes());
    for b in &mut out[start + 5..end] {
        *b = 0x00; // nop
    }
    Ok(())
}

fn build_il(action: &Action) -> Option<Vec<u8>> {
    match action {
        Action::ReturnTrue => Some(vec![0x17, 0x2A]), // ldc.i4.1; ret
        Action::ReturnFalse => Some(vec![0x16, 0x2A]), // ldc.i4.0; ret
        Action::ReturnVoid => Some(vec![0x2A]),       // ret
        Action::ReturnInt(v) => {
            let mut out = vec![0x20];
            out.extend_from_slice(&v.to_le_bytes());
            out.push(0x2A);
            Some(out)
        }
        Action::Pcl2UnlockDownload => None,
    }
}

pub fn patch_pe(bytes: &[u8], rules: &[Rule]) -> Result<(Vec<u8>, Report)> {
    let mut report = Report::default();
    let mut out = bytes.to_vec();
    let mut patched_offsets = HashSet::new();
    let mut candidates = candidate_bases(bytes);
    if !candidates.contains(&0) {
        candidates.insert(0);
    }

    let mut assemblies = 0usize;
    for base in candidates {
        let Ok(pe) = PeImage::parse(bytes, base) else {
            continue;
        };
        let Some(meta_off) = pe.meta_off else {
            continue;
        };
        let Ok(meta) = Metadata::parse(bytes, meta_off) else {
            continue;
        };
        assemblies += 1;
        let targets = collect_targets(bytes, &meta, &pe, rules);
        for t in targets {
            let Some(code_off) = pe.rva_to_off(t.rva) else {
                continue;
            };
            if !patched_offsets.insert(code_off) {
                continue;
            }
            let Some((body_off, body_size, flags_off, max_stack_off)) =
                method_code(bytes, code_off)
            else {
                report
                    .skipped
                    .push(format!("{}.{} — 无法读取方法体", t.class, t.method));
                continue;
            };
            if t.action == Action::Pcl2UnlockDownload {
                match patch_pcl2_loader(&mut out, &meta, body_off, body_size) {
                    Ok(()) => {
                        report.applied.push(Applied {
                            class: t.class,
                            method: t.method,
                            desc: t.desc,
                            action: t.action,
                            note: t.note,
                        });
                    }
                    Err(e) => report
                        .skipped
                        .push(format!("{}.{} — {e}", t.class, t.method)),
                }
                continue;
            }
            let Some(new_il) = build_il(&t.action) else {
                continue;
            };
            if new_il.len() > body_size {
                report.skipped.push(format!(
                    "{}.{} — 方法体太短 ({} < {})",
                    t.class,
                    t.method,
                    body_size,
                    new_il.len()
                ));
                continue;
            }
            // The CLR requires the final byte of the body to be a flow
            // terminator, so put the return sequence at the end and pad the
            // front with nops (same trick as the Java backend).
            let start = body_off + body_size - new_il.len();
            for b in &mut out[body_off..start] {
                *b = 0x00; // nop
            }
            out[start..start + new_il.len()].copy_from_slice(&new_il);
            if let (Some(f), Some(ms)) = (flags_off, max_stack_off) {
                let flags = u16::from_le_bytes([out[f], out[f + 1]]);
                let flags = flags & !0x0008; // CorILMethod_MoreSects: drop EH clauses
                out[f..f + 2].copy_from_slice(&flags.to_le_bytes());
                let old = u16::from_le_bytes([out[ms], out[ms + 1]]);
                let new = old.max(1);
                out[ms..ms + 2].copy_from_slice(&new.to_le_bytes());
            }
            report.applied.push(Applied {
                class: t.class,
                method: t.method,
                desc: t.desc,
                action: t.action,
                note: t.note,
            });
        }
    }

    if assemblies == 0 {
        report
            .notes
            .push("没有找到 .NET 元数据（可能是已压缩的 single-file，或不是 .NET 程序）".into());
    } else {
        report
            .notes
            .push(format!("扫描到 {assemblies} 个 .NET 程序集"));
    }

    if report.applied.is_empty() {
        return Ok((bytes.to_vec(), report));
    }
    Ok((out, report))
}

fn candidate_bases(bytes: &[u8]) -> HashSet<usize> {
    let mut bases = HashSet::new();
    let mut i = 0usize;
    // A 256 MiB cap keeps a pathological input from stalling the UI thread.
    // Large self-contained .NET bundles can contain well over 64 assemblies.
    let limit = bytes.len().min(256 * 1024 * 1024);
    while i + 2 <= limit {
        if bytes[i] == b'M' && bytes[i + 1] == b'Z' {
            if PeImage::parse(bytes, i).is_ok() {
                bases.insert(i);
                if bases.len() >= 256 {
                    break;
                }
            }
        }
        i += 1;
    }
    bases
}

/// Best-effort assembly identity, used by target detection for single-file exes.
pub fn probe_assembly_version(bytes: &[u8]) -> Option<String> {
    let mut candidates = candidate_bases(bytes);
    candidates.insert(0);
    let mut fallback = None;
    let mut best: Option<((u64, u64, u64, u64), String)> = None;
    for base in candidates {
        let Ok(pe) = PeImage::parse(bytes, base) else {
            continue;
        };
        let Some(meta_off) = pe.meta_off else {
            continue;
        };
        let Ok(meta) = Metadata::parse(bytes, meta_off) else {
            continue;
        };
        let Ok((name, version)) = meta.assembly_name_version(bytes) else {
            continue;
        };
        let lower = name.to_ascii_lowercase();
        let is_main = lower == "plain craft launcher 2"
            || lower == "plain craft launcher"
            || lower == "pcl"
            || lower == "pcl2"
            || lower == "pcl2-ce"
            || lower == "pcl-ce"
            || lower == "hmcl"
            || lower.starts_with("plain craft launcher");
        if is_main {
            return Some(version);
        }
        // PCL.Core also contains "pcl" and reports 1.0.0.0; never let a
        // dependency win over the launcher assembly. Keep the highest
        // version among real PCL/HMCL-named candidates as a fallback.
        if lower.contains("pcl") || lower.contains("hmcl") || lower.contains("plain craft") {
            let key = version_key(&version);
            if best.as_ref().map(|(k, _)| key > *k).unwrap_or(true) {
                best = Some((key, version.clone()));
            }
        }
        if fallback.is_none() && version != "0.0.0.0" {
            fallback = Some(version);
        }
    }
    best.map(|(_, v)| v).or(fallback)
}

fn version_key(v: &str) -> (u64, u64, u64, u64) {
    let mut out = [0u64; 4];
    for (i, part) in v.split('.').take(4).enumerate() {
        out[i] = part.parse().unwrap_or(0);
    }
    (out[0], out[1], out[2], out[3])
}

fn read(bytes: &[u8], off: usize, n: usize) -> Result<&[u8]> {
    let end = off.checked_add(n).context("offset overflow")?;
    bytes
        .get(off..end)
        .with_context(|| format!("out of bounds at {off:#x}"))
}

fn u16_at(bytes: &[u8], off: usize) -> Result<u16> {
    let s = read(bytes, off, 2)?;
    Ok(u16::from_le_bytes([s[0], s[1]]))
}

fn u32_at(bytes: &[u8], off: usize) -> Result<u32> {
    let s = read(bytes, off, 4)?;
    Ok(u32::from_le_bytes([s[0], s[1], s[2], s[3]]))
}

fn read_uint(bytes: &[u8], off: usize, size: usize) -> Result<u32> {
    match size {
        2 => Ok(u16_at(bytes, off)? as u32),
        4 => u32_at(bytes, off),
        _ => bail!("unsupported integer size {size}"),
    }
}

fn read_compressed(bytes: &[u8], p: &mut usize) -> Result<u32> {
    let b0 = *bytes.get(*p).context("compressed integer out of bounds")?;
    *p += 1;
    if b0 & 0x80 == 0 {
        Ok(b0 as u32)
    } else if b0 & 0xC0 == 0x80 {
        let b1 = *bytes.get(*p).context("compressed integer out of bounds")?;
        *p += 1;
        Ok((((b0 & 0x3F) as u32) << 8) | b1 as u32)
    } else if b0 & 0xE0 == 0xC0 {
        let b1 = *bytes.get(*p).context("compressed integer out of bounds")?;
        let b2 = *bytes
            .get(*p + 1)
            .context("compressed integer out of bounds")?;
        let b3 = *bytes
            .get(*p + 2)
            .context("compressed integer out of bounds")?;
        *p += 3;
        Ok((((b0 & 0x1F) as u32) << 24) | ((b1 as u32) << 16) | ((b2 as u32) << 8) | b3 as u32)
    } else {
        bail!("invalid compressed integer")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn compressed() {
        let mut p = 0;
        assert_eq!(read_compressed(&[0x03], &mut p).unwrap(), 3);
        let mut p = 0;
        assert_eq!(read_compressed(&[0x80, 0x01], &mut p).unwrap(), 1);
    }

    #[test]
    fn tiny_body() {
        let b = [0x06u8]; // tiny, size 1
        let (off, size, f, m) = method_code(&b, 0).unwrap();
        assert_eq!((off, size, f, m), (1, 1, None, None));
    }
}
