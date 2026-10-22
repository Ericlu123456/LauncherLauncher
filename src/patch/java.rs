//! Minimal Java class-file reader and in-place method-body rewriter.
//!
//! Only the parts needed to locate a `Code` attribute are parsed. The rewritten
//! body is always shorter than the original, so the class file layout (constant
//! pool, attribute lengths, StackMapTable) stays valid without reserialisation.

use super::{Action, Report, Rule, rule_matches};
use anyhow::{Context, Result, bail};

#[allow(dead_code)]
#[derive(Clone, Debug)]
enum Cp {
    Utf8(String),
    Class(u16),
    String(u16),
    NameType(u16, u16),
    MethodRef(u16, u16),
    FieldRef(u16, u16),
    InterfaceMethodRef(u16, u16),
    Other,
}

#[derive(Clone, Debug)]
struct Code {
    max_stack_off: usize,
    code_len: usize,
    code_off: usize,
    exception_len_off: usize,
    /// Offset of the Code attribute's own u32 `attribute_length`.
    attr_len_off: usize,
    /// First exception-table entry, and the number of entries.
    eh_start: usize,
    eh_len: usize,
    stack_map_name_off: Option<usize>,
}

#[derive(Clone, Debug)]
struct Method {
    name: String,
    desc: String,
    desc_idx: u16,
    code: Option<Code>,
}

#[derive(Debug)]
struct ClassFile {
    bytes: Vec<u8>,
    this_class: String,
    methods: Vec<Method>,
}

struct R<'a> {
    b: &'a [u8],
    p: usize,
}

impl<'a> R<'a> {
    fn new(b: &'a [u8]) -> Self {
        Self { b, p: 0 }
    }

    fn u8(&mut self) -> Result<u8> {
        if self.p >= self.b.len() {
            bail!("class 文件意外结束");
        }
        let v = self.b[self.p];
        self.p += 1;
        Ok(v)
    }

    fn u16(&mut self) -> Result<u16> {
        let s = self.take(2)?;
        Ok(u16::from_be_bytes([s[0], s[1]]))
    }

    fn u32(&mut self) -> Result<u32> {
        let s = self.take(4)?;
        Ok(u32::from_be_bytes([s[0], s[1], s[2], s[3]]))
    }

    fn take(&mut self, n: usize) -> Result<&'a [u8]> {
        let end = self.p.checked_add(n).context("class 偏移溢出")?;
        if end > self.b.len() {
            bail!(
                "class 文件意外结束 (需要 {} 字节, 剩余 {})",
                n,
                self.b.len() - self.p
            );
        }
        let s = &self.b[self.p..end];
        self.p = end;
        Ok(s)
    }

    fn skip(&mut self, n: usize) -> Result<()> {
        self.take(n).map(|_| ())
    }
}

impl ClassFile {
    fn parse(bytes: &[u8]) -> Result<Self> {
        let mut r = R::new(bytes);
        if r.u32()? != 0xCAFE_BABE {
            bail!("不是 Java class 文件");
        }
        let _minor = r.u16()?;
        let _major = r.u16()?;

        let cp_count = r.u16()? as usize;
        let mut cp = vec![Cp::Other; cp_count.max(1)];
        let mut i = 1usize;
        while i < cp_count {
            let tag = r.u8()?;
            match tag {
                1 => {
                    let len = r.u16()? as usize;
                    let s = r.take(len)?;
                    cp[i] = Cp::Utf8(String::from_utf8_lossy(s).into_owned());
                }
                3 | 4 => r.skip(4)?,
                5 | 6 => {
                    r.skip(8)?;
                    i += 1;
                }
                7 => cp[i] = Cp::Class(r.u16()?),
                8 => cp[i] = Cp::String(r.u16()?),
                9 => cp[i] = Cp::FieldRef(r.u16()?, r.u16()?),
                10 => cp[i] = Cp::MethodRef(r.u16()?, r.u16()?),
                11 => cp[i] = Cp::InterfaceMethodRef(r.u16()?, r.u16()?),
                12 => cp[i] = Cp::NameType(r.u16()?, r.u16()?),
                15 => r.skip(3)?,
                16 => r.skip(2)?,
                17 | 18 => r.skip(4)?,
                19 | 20 => r.skip(2)?,
                other => bail!("未知常量池 tag {other}"),
            }
            i += 1;
        }

        let _access = r.u16()?;
        let this_idx = r.u16()? as usize;
        let _super_idx = r.u16()?;
        let interfaces = r.u16()? as usize;
        r.skip(interfaces * 2)?;

        let fields = r.u16()? as usize;
        for _ in 0..fields {
            skip_member(&mut r)?;
        }

        let method_count = r.u16()? as usize;
        let mut methods = Vec::with_capacity(method_count);
        for _ in 0..method_count {
            let _access = r.u16()?;
            let name_idx = r.u16()? as usize;
            let desc_idx = r.u16()? as usize;
            let attr_count = r.u16()? as usize;
            let name = utf8(&cp, name_idx)?.to_string();
            let desc = utf8(&cp, desc_idx)?.to_string();
            let desc_idx = desc_idx as u16;
            let mut code = None;
            for _ in 0..attr_count {
                let an = r.u16()? as usize;
                let alen = r.u32()? as usize;
                let astart = r.p;
                let aend = astart.checked_add(alen).context("属性长度溢出")?;
                if aend > bytes.len() {
                    bail!("class 属性越界");
                }
                if utf8(&cp, an)? == "Code" {
                    let mut cr = R {
                        b: &bytes[astart..aend],
                        p: 0,
                    };
                    code = Some(parse_code(&cp, &mut cr, astart)?);
                }
                r.p = aend;
            }
            methods.push(Method {
                name,
                desc,
                desc_idx,
                code,
            });
        }

        let this_class = match cp.get(this_idx) {
            Some(Cp::Class(n)) => utf8(&cp, *n as usize)?.to_string(),
            _ => bail!("this_class 无效"),
        };

        Ok(Self {
            bytes: bytes.to_vec(),
            this_class,
            methods,
        })
    }
}

fn utf8(cp: &[Cp], idx: usize) -> Result<&str> {
    match cp.get(idx) {
        Some(Cp::Utf8(s)) => Ok(s),
        _ => bail!("常量池索引 {idx} 不是 Utf8"),
    }
}

fn skip_member(r: &mut R<'_>) -> Result<()> {
    r.skip(2)?; // access
    r.skip(2)?; // name
    r.skip(2)?; // descriptor
    let attrs = r.u16()? as usize;
    for _ in 0..attrs {
        r.skip(2)?;
        let len = r.u32()? as usize;
        r.skip(len)?;
    }
    Ok(())
}

/// `base` is the offset of the Code attribute's info inside the whole class.
fn parse_code(cp: &[Cp], r: &mut R<'_>, base: usize) -> Result<Code> {
    let max_stack_off = base + r.p;
    let _max_stack = r.u16()?;
    let _max_locals = r.u16()?;
    let code_len = r.u32()? as usize;
    let code_off = base + r.p;
    r.skip(code_len)?;
    let exception_len_off = base + r.p;
    let eh_len = r.u16()? as usize;
    let eh_start = base + r.p;
    r.skip(eh_len * 8)?;
    let nested = r.u16()? as usize;
    let mut stack_map_name_off = None;
    for _ in 0..nested {
        let name_off = base + r.p;
        let an = r.u16()? as usize;
        let len = r.u32()? as usize;
        if utf8(cp, an)? == "StackMapTable" {
            stack_map_name_off = Some(name_off);
        }
        r.skip(len)?;
    }
    Ok(Code {
        max_stack_off,
        code_len,
        code_off,
        exception_len_off,
        attr_len_off: base.saturating_sub(4),
        eh_start,
        eh_len,
        stack_map_name_off,
    })
}

fn build_code(action: &Action, desc: &str) -> Option<Vec<u8>> {
    match action {
        Action::ReturnTrue if desc.ends_with(")Z") => Some(vec![0x04, 0xAC]),
        Action::ReturnFalse if desc.ends_with(")Z") => Some(vec![0x03, 0xAC]),
        Action::ReturnVoid if desc.ends_with(")V") => Some(vec![0xB1]),
        Action::ReturnInt(v) if desc.ends_with(")I") => {
            let mut out = Vec::with_capacity(6);
            let v = *v;
            if (-1..=5).contains(&v) {
                out.push((v + 3) as u8);
            } else if i8::try_from(v).is_ok() {
                out.push(0x10);
                out.push(v as i8 as u8);
            } else if i16::try_from(v).is_ok() {
                out.push(0x11);
                out.extend_from_slice(&(v as i16).to_be_bytes());
            } else {
                return None;
            }
            out.push(0xAC);
            Some(out)
        }
        _ => None,
    }
}

/// Patch one class. Returns `None` when no rule matched.
pub fn patch_class(bytes: &[u8], rules: &[Rule], report: &mut Report) -> Result<Option<Vec<u8>>> {
    let class = ClassFile::parse(bytes)?;
    let mut out = class.bytes.clone();
    let mut changed = false;

    // Reverse order: removing an exception table shrinks the class, and doing
    // the later methods first keeps the earlier methods' offsets valid.
    for method in class.methods.iter().rev() {
        if method.name == "<init>" || method.name == "<clinit>" {
            continue;
        }
        let Some(code) = &method.code else {
            continue;
        };
        let Some(rule) = rules
            .iter()
            .find(|r| rule_matches(r, &class.this_class, &method.name, &method.desc))
        else {
            continue;
        };
        let Some(new_code) = build_code(&rule.action, &method.desc) else {
            report.skipped.push(format!(
                "{}.{}{} — 返回类型与 {:?} 不兼容",
                class.this_class, method.name, method.desc, rule.action
            ));
            continue;
        };
        if new_code.len() > code.code_len {
            report.skipped.push(format!(
                "{}.{}{} — 原方法体太短 ({} < {})",
                class.this_class,
                method.name,
                method.desc,
                code.code_len,
                new_code.len()
            ));
            continue;
        }

        // Put the return sequence at the *end* of the old body and pad the
        // front with nops. A trailing return would start a new basic block at
        // the next byte, which the split verifier rejects without a frame.
        let start = code.code_off + code.code_len - new_code.len();
        for b in &mut out[code.code_off..start] {
            *b = 0x00; // nop
        }
        out[start..start + new_code.len()].copy_from_slice(&new_code);

        // If the method had branch frames, rename the StackMapTable attribute
        // to the method descriptor. Unknown attributes are ignored, and the
        // rewritten body has no branches that would need frames.
        if let Some(off) = code.stack_map_name_off {
            out[off..off + 2].copy_from_slice(&method.desc_idx.to_be_bytes());
        }

        // Drop the exception table instead of leaving its bytes in place:
        // the Code attribute is a variable-length structure, so keeping the
        // entries while reporting length 0 makes the JVM parse them as
        // nested attributes ("Truncated class file").
        if code.eh_len > 0 {
            let bytes = code.eh_len * 8;
            let end = code.eh_start + bytes;
            if end <= out.len() {
                out.drain(code.eh_start..end);
                let off = code.attr_len_off;
                if off + 4 <= out.len() {
                    let old =
                        u32::from_be_bytes([out[off], out[off + 1], out[off + 2], out[off + 3]]);
                    let new = old.saturating_sub(bytes as u32);
                    out[off..off + 4].copy_from_slice(&new.to_be_bytes());
                }
            }
        }

        let old_stack = u16::from_be_bytes([out[code.max_stack_off], out[code.max_stack_off + 1]]);
        let new_stack = old_stack.max(2);
        out[code.max_stack_off..code.max_stack_off + 2].copy_from_slice(&new_stack.to_be_bytes());
        out[code.exception_len_off..code.exception_len_off + 2].copy_from_slice(&[0, 0]);

        report.applied.push(super::Applied {
            class: class.this_class.clone(),
            method: method.name.clone(),
            desc: method.desc.clone(),
            action: rule.action.clone(),
            note: rule.note.clone(),
        });
        changed = true;
    }

    Ok(changed.then_some(out))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn opcode_shapes() {
        assert_eq!(build_code(&Action::ReturnFalse, "()Z"), Some(vec![3, 0xAC]));
        assert_eq!(build_code(&Action::ReturnVoid, "()V"), Some(vec![0xB1]));
        assert_eq!(build_code(&Action::ReturnTrue, "()V"), None);
    }
}
