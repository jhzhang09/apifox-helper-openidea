//! 纯 Rust ZIP/DEFLATE 读取器与 JVM `.class` 字节码解析器，用于按需解析本地依赖 JAR 中的类型契约。
use crate::source::{Annotation, Class, EnumVariant, Field, annotation};
use std::collections::{BTreeMap, BTreeSet, HashMap, HashSet};

pub const DEFAULT_MAX_ENTRY_BYTES: usize = 4 * 1024 * 1024;

#[derive(Clone, Copy, Debug)]
struct ZipEntry {
    method: u16,
    compressed_size: u32,
    uncompressed_size: u32,
    local_header_offset: u32,
}

#[derive(Clone, Debug)]
pub struct ZipIndex {
    entries: HashMap<String, ZipEntry>,
}

pub struct ZipArchive<'a> {
    bytes: &'a [u8],
    index: ZipIndex,
}

fn le_u16(bytes: &[u8], offset: usize) -> Option<u16> {
    bytes
        .get(offset..offset + 2)
        .map(|s| u16::from_le_bytes([s[0], s[1]]))
}

fn le_u32(bytes: &[u8], offset: usize) -> Option<u32> {
    bytes
        .get(offset..offset + 4)
        .map(|s| u32::from_le_bytes([s[0], s[1], s[2], s[3]]))
}

impl ZipIndex {
    pub fn parse(bytes: &[u8]) -> Option<Self> {
        if bytes.len() < 22 {
            return None;
        }
        let min_pos = bytes.len().saturating_sub(22 + 65535);
        let mut eocd = None;
        for pos in (min_pos..=bytes.len() - 22).rev() {
            if bytes[pos..pos + 4] == *b"PK\x05\x06" {
                let comment_len = le_u16(bytes, pos + 20)? as usize;
                if pos + 22 + comment_len == bytes.len() {
                    eocd = Some(pos);
                    break;
                }
            }
        }
        let eocd = eocd?;
        let total_entries = le_u16(bytes, eocd + 10)? as usize;
        let cd_size = le_u32(bytes, eocd + 12)? as usize;
        let cd_offset = le_u32(bytes, eocd + 16)? as usize;
        if cd_offset.checked_add(cd_size)? > eocd {
            return None;
        }
        let mut entries = HashMap::with_capacity(total_entries);
        let mut pos = cd_offset;
        let cd_end = cd_offset + cd_size;
        for _ in 0..total_entries {
            if pos + 46 > cd_end || bytes[pos..pos + 4] != *b"PK\x01\x02" {
                return None;
            }
            let method = le_u16(bytes, pos + 10)?;
            let compressed_size = le_u32(bytes, pos + 20)?;
            let uncompressed_size = le_u32(bytes, pos + 24)?;
            let name_len = le_u16(bytes, pos + 28)? as usize;
            let extra_len = le_u16(bytes, pos + 30)? as usize;
            let comment_len = le_u16(bytes, pos + 32)? as usize;
            let local_header_offset = le_u32(bytes, pos + 42)?;
            let name_start = pos + 46;
            let name_end = name_start.checked_add(name_len)?;
            let next_pos = name_end.checked_add(extra_len)?.checked_add(comment_len)?;
            if next_pos > cd_end {
                return None;
            }
            let name = String::from_utf8_lossy(&bytes[name_start..name_end]).into_owned();
            entries.insert(
                name,
                ZipEntry {
                    method,
                    compressed_size,
                    uncompressed_size,
                    local_header_offset,
                },
            );
            pos = next_pos;
        }
        Some(Self { entries })
    }

    pub fn contains(&self, name: &str) -> bool {
        self.entries.contains_key(name)
    }

    pub fn entry_names(&self) -> impl Iterator<Item = &str> {
        self.entries.keys().map(String::as_str)
    }

    pub fn read_entry(&self, bytes: &[u8], name: &str, max_bytes: usize) -> Option<Vec<u8>> {
        let entry = self.entries.get(name)?;
        if entry.uncompressed_size as usize > max_bytes {
            return None;
        }
        let lfh = entry.local_header_offset as usize;
        if lfh + 30 > bytes.len() || bytes[lfh..lfh + 4] != *b"PK\x03\x04" {
            return None;
        }
        let name_len = le_u16(bytes, lfh + 26)? as usize;
        let extra_len = le_u16(bytes, lfh + 28)? as usize;
        let data_start = lfh
            .checked_add(30)?
            .checked_add(name_len)?
            .checked_add(extra_len)?;
        let data_end = data_start.checked_add(entry.compressed_size as usize)?;
        if data_end > bytes.len() {
            return None;
        }
        let payload = &bytes[data_start..data_end];
        match entry.method {
            0 => {
                if payload.len() > max_bytes {
                    return None;
                }
                Some(payload.to_vec())
            }
            8 => inflate_raw(payload, max_bytes).ok(),
            _ => None,
        }
    }
}

impl<'a> ZipArchive<'a> {
    pub fn parse(bytes: &'a [u8]) -> Option<Self> {
        Some(Self {
            bytes,
            index: ZipIndex::parse(bytes)?,
        })
    }

    pub fn contains(&self, name: &str) -> bool {
        self.index.contains(name)
    }

    pub fn entry_names(&self) -> impl Iterator<Item = &str> {
        self.index.entry_names()
    }

    pub fn read_entry(&self, name: &str, max_bytes: usize) -> Option<Vec<u8>> {
        self.index.read_entry(self.bytes, name, max_bytes)
    }
}

struct BitReader<'a> {
    bytes: &'a [u8],
    pos: usize,
    buf: u32,
    cnt: u8,
}

impl<'a> BitReader<'a> {
    fn new(bytes: &'a [u8]) -> Self {
        Self {
            bytes,
            pos: 0,
            buf: 0,
            cnt: 0,
        }
    }

    fn ensure(&mut self, n: u8) -> Result<(), &'static str> {
        while self.cnt < n {
            if self.pos >= self.bytes.len() {
                return Err("unexpected end of deflate stream");
            }
            self.buf |= (self.bytes[self.pos] as u32) << self.cnt;
            self.pos += 1;
            self.cnt += 8;
        }
        Ok(())
    }

    fn read_bits(&mut self, n: u8) -> Result<u32, &'static str> {
        if n == 0 {
            return Ok(0);
        }
        self.ensure(n)?;
        let mask = (1u32 << n) - 1;
        let val = self.buf & mask;
        self.buf >>= n;
        self.cnt -= n;
        Ok(val)
    }

    fn align_byte(&mut self) {
        let drop = self.cnt % 8;
        self.cnt -= drop;
        self.pos = self.pos.saturating_sub((self.cnt / 8) as usize);
        self.buf = 0;
        self.cnt = 0;
    }

    fn read_u16_le(&mut self) -> Result<u16, &'static str> {
        if self.pos + 2 > self.bytes.len() {
            return Err("unexpected end of stored block");
        }
        let val = u16::from_le_bytes([self.bytes[self.pos], self.bytes[self.pos + 1]]);
        self.pos += 2;
        Ok(val)
    }

    fn read_bytes(&mut self, len: usize) -> Result<&'a [u8], &'static str> {
        if self.pos + len > self.bytes.len() {
            return Err("unexpected end of stored block payload");
        }
        let slice = &self.bytes[self.pos..self.pos + len];
        self.pos += len;
        Ok(slice)
    }
}

struct HuffmanTable {
    counts: [u16; 16],
    symbols: Vec<u16>,
}

impl HuffmanTable {
    fn from_lengths(lengths: &[u8]) -> Result<Self, &'static str> {
        let mut counts = [0u16; 16];
        for &len in lengths {
            if len as usize > 15 {
                return Err("huffman code length > 15");
            }
            if len > 0 {
                counts[len as usize] += 1;
            }
        }
        let mut offsets = [0u16; 16];
        let mut total = 0u16;
        for len in 1..=15 {
            offsets[len] = total;
            total += counts[len];
        }
        let mut symbols = vec![0u16; total as usize];
        for (sym, &len) in lengths.iter().enumerate() {
            if len > 0 {
                let idx = offsets[len as usize] as usize;
                offsets[len as usize] += 1;
                symbols[idx] = sym as u16;
            }
        }
        Ok(Self { counts, symbols })
    }

    fn decode(&self, r: &mut BitReader<'_>) -> Result<u16, &'static str> {
        let mut code: i32 = 0;
        let mut first: i32 = 0;
        let mut index: i32 = 0;
        for len in 1..=15 {
            code |= r.read_bits(1)? as i32;
            let count = self.counts[len] as i32;
            if code - count < first {
                return Ok(self.symbols[(index + (code - first)) as usize]);
            }
            index += count;
            first += count;
            first <<= 1;
            code <<= 1;
        }
        Err("invalid huffman code")
    }
}

fn fixed_tables() -> Result<(HuffmanTable, HuffmanTable), &'static str> {
    let mut lit_lengths = [0u8; 288];
    lit_lengths[..144].fill(8);
    lit_lengths[144..256].fill(9);
    lit_lengths[256..280].fill(7);
    lit_lengths[280..288].fill(8);
    let dist_lengths = [5u8; 32];
    Ok((
        HuffmanTable::from_lengths(&lit_lengths)?,
        HuffmanTable::from_lengths(&dist_lengths)?,
    ))
}

fn dynamic_tables(r: &mut BitReader<'_>) -> Result<(HuffmanTable, HuffmanTable), &'static str> {
    const CL_ORDER: [usize; 19] = [
        16, 17, 18, 0, 8, 7, 9, 6, 10, 5, 11, 4, 12, 3, 13, 2, 14, 1, 15,
    ];
    let hlit = r.read_bits(5)? as usize + 257;
    let hdist = r.read_bits(5)? as usize + 1;
    let hclen = r.read_bits(4)? as usize + 4;
    if hlit > 286 || hdist > 30 {
        return Err("invalid dynamic huffman header");
    }
    let mut cl_lengths = [0u8; 19];
    for i in 0..hclen {
        cl_lengths[CL_ORDER[i]] = r.read_bits(3)? as u8;
    }
    let cl_table = HuffmanTable::from_lengths(&cl_lengths)?;
    let total = hlit + hdist;
    let mut lengths = Vec::with_capacity(total);
    while lengths.len() < total {
        let sym = cl_table.decode(r)?;
        match sym {
            0..=15 => lengths.push(sym as u8),
            16 => {
                let prev = *lengths.last().ok_or("repeat with no previous length")?;
                let repeat = r.read_bits(2)? as usize + 3;
                if lengths.len() + repeat > total {
                    return Err("code length overflow");
                }
                lengths.resize(lengths.len() + repeat, prev);
            }
            17 => {
                let repeat = r.read_bits(3)? as usize + 3;
                if lengths.len() + repeat > total {
                    return Err("code length overflow");
                }
                lengths.resize(lengths.len() + repeat, 0);
            }
            18 => {
                let repeat = r.read_bits(7)? as usize + 11;
                if lengths.len() + repeat > total {
                    return Err("code length overflow");
                }
                lengths.resize(lengths.len() + repeat, 0);
            }
            _ => return Err("invalid code length symbol"),
        }
    }
    Ok((
        HuffmanTable::from_lengths(&lengths[..hlit])?,
        HuffmanTable::from_lengths(&lengths[hlit..])?,
    ))
}

pub fn inflate_raw(input: &[u8], max_out: usize) -> Result<Vec<u8>, &'static str> {
    const LEN_BASE: [usize; 29] = [
        3, 4, 5, 6, 7, 8, 9, 10, 11, 13, 15, 17, 19, 23, 27, 31, 35, 43, 51, 59, 67, 83, 99, 115,
        131, 163, 195, 227, 258,
    ];
    const LEN_EXTRA: [u8; 29] = [
        0, 0, 0, 0, 0, 0, 0, 0, 1, 1, 1, 1, 2, 2, 2, 2, 3, 3, 3, 3, 4, 4, 4, 4, 5, 5, 5, 5, 0,
    ];
    const DIST_BASE: [usize; 30] = [
        1, 2, 3, 4, 5, 7, 9, 13, 17, 25, 33, 49, 65, 97, 129, 193, 257, 385, 513, 769, 1025, 1537,
        2049, 3073, 4097, 6145, 8193, 12289, 16385, 24577,
    ];
    const DIST_EXTRA: [u8; 30] = [
        0, 0, 0, 0, 1, 1, 2, 2, 3, 3, 4, 4, 5, 5, 6, 6, 7, 7, 8, 8, 9, 9, 10, 10, 11, 11, 12, 12,
        13, 13,
    ];

    let mut r = BitReader::new(input);
    let mut out = Vec::new();
    loop {
        let bfinal = r.read_bits(1)?;
        let btype = r.read_bits(2)?;
        match btype {
            0 => {
                r.align_byte();
                let len = r.read_u16_le()?;
                let nlen = r.read_u16_le()?;
                if len != !nlen {
                    return Err("stored block length mismatch");
                }
                let len = len as usize;
                if out.len() + len > max_out {
                    return Err("decompressed size exceeds limit");
                }
                out.extend_from_slice(r.read_bytes(len)?);
            }
            1 | 2 => {
                let (lit_table, dist_table) = if btype == 1 {
                    fixed_tables()?
                } else {
                    dynamic_tables(&mut r)?
                };
                loop {
                    let sym = lit_table.decode(&mut r)?;
                    if sym < 256 {
                        if out.len() >= max_out {
                            return Err("decompressed size exceeds limit");
                        }
                        out.push(sym as u8);
                    } else if sym == 256 {
                        break;
                    } else {
                        let len_idx = (sym as usize).checked_sub(257).ok_or("invalid length")?;
                        if len_idx >= LEN_BASE.len() {
                            return Err("invalid length symbol");
                        }
                        let length = LEN_BASE[len_idx] + r.read_bits(LEN_EXTRA[len_idx])? as usize;
                        let dist_sym = dist_table.decode(&mut r)? as usize;
                        if dist_sym >= DIST_BASE.len() {
                            return Err("invalid distance symbol");
                        }
                        let dist =
                            DIST_BASE[dist_sym] + r.read_bits(DIST_EXTRA[dist_sym])? as usize;
                        if dist == 0 || dist > out.len() {
                            return Err("invalid back-reference distance");
                        }
                        if out.len() + length > max_out {
                            return Err("decompressed size exceeds limit");
                        }
                        for _ in 0..length {
                            let b = out[out.len() - dist];
                            out.push(b);
                        }
                    }
                }
            }
            _ => return Err("reserved deflate block type"),
        }
        if bfinal != 0 {
            break;
        }
    }
    Ok(out)
}

struct ClassReader<'a> {
    bytes: &'a [u8],
    pos: usize,
}

impl<'a> ClassReader<'a> {
    fn new(bytes: &'a [u8]) -> Self {
        Self { bytes, pos: 0 }
    }
    fn u8(&mut self) -> Option<u8> {
        let b = *self.bytes.get(self.pos)?;
        self.pos += 1;
        Some(b)
    }
    fn u16(&mut self) -> Option<u16> {
        let s = self.bytes.get(self.pos..self.pos + 2)?;
        self.pos += 2;
        Some(u16::from_be_bytes([s[0], s[1]]))
    }
    fn u32(&mut self) -> Option<u32> {
        let s = self.bytes.get(self.pos..self.pos + 4)?;
        self.pos += 4;
        Some(u32::from_be_bytes([s[0], s[1], s[2], s[3]]))
    }
    fn i32(&mut self) -> Option<i32> {
        self.u32().map(|v| v as i32)
    }
    fn i64(&mut self) -> Option<i64> {
        let s = self.bytes.get(self.pos..self.pos + 8)?;
        self.pos += 8;
        Some(i64::from_be_bytes([
            s[0], s[1], s[2], s[3], s[4], s[5], s[6], s[7],
        ]))
    }
    fn f32(&mut self) -> Option<f32> {
        self.u32().map(f32::from_bits)
    }
    fn f64(&mut self) -> Option<f64> {
        self.i64().map(|v| f64::from_bits(v as u64))
    }
    fn bytes(&mut self, len: usize) -> Option<&'a [u8]> {
        let s = self.bytes.get(self.pos..self.pos + len)?;
        self.pos += len;
        Some(s)
    }
    fn skip(&mut self, len: usize) -> Option<()> {
        self.bytes(len).map(|_| ())
    }
}

#[derive(Clone, Debug)]
enum CpEntry {
    Placeholder,
    Utf8(String),
    Class(u16),
    Int(i32),
    Float(f32),
    Long(i64),
    Double(f64),
    Other,
}

fn decode_mutf8(raw: &[u8]) -> String {
    if !raw.contains(&0xC0) {
        return String::from_utf8_lossy(raw).into_owned();
    }
    let mut normalized = Vec::with_capacity(raw.len());
    let mut i = 0;
    while i < raw.len() {
        if raw[i] == 0xC0 && raw.get(i + 1) == Some(&0x80) {
            normalized.push(0);
            i += 2;
        } else {
            normalized.push(raw[i]);
            i += 1;
        }
    }
    String::from_utf8_lossy(&normalized).into_owned()
}

fn cp_utf8(cp: &[CpEntry], idx: u16) -> Option<&str> {
    match cp.get(idx as usize)? {
        CpEntry::Utf8(s) => Some(s.as_str()),
        _ => None,
    }
}

fn cp_class_name(cp: &[CpEntry], idx: u16) -> Option<&str> {
    match cp.get(idx as usize)? {
        CpEntry::Class(name_idx) => cp_utf8(cp, *name_idx),
        _ => None,
    }
}

fn split_internal_name(internal: &str) -> (String, String) {
    if let Some((pkg, cls)) = internal.rsplit_once('/') {
        (pkg.replace('/', "."), cls.replace('$', "."))
    } else {
        (String::new(), internal.replace('$', "."))
    }
}

fn format_internal_qualified(internal: &str) -> String {
    let (pkg, cls) = split_internal_name(internal);
    if pkg.is_empty() {
        cls
    } else {
        format!("{pkg}.{cls}")
    }
}

fn parse_type_args(mut s: &str) -> Option<(Vec<String>, &str)> {
    s = s.strip_prefix('<')?;
    let mut args = Vec::new();
    while !s.starts_with('>') {
        if let Some(rest) = s.strip_prefix('*') {
            args.push("?".to_string());
            s = rest;
        } else if let Some(rest) = s.strip_prefix('+') {
            let (ty, next) = parse_type_sig(rest)?;
            args.push(ty);
            s = next;
        } else if let Some(rest) = s.strip_prefix('-') {
            let (ty, next) = parse_type_sig(rest)?;
            args.push(ty);
            s = next;
        } else {
            let (ty, next) = parse_type_sig(s)?;
            args.push(ty);
            s = next;
        }
    }
    Some((args, &s[1..]))
}

fn parse_class_type_sig(mut s: &str) -> Option<(String, &str)> {
    s = s.strip_prefix('L')?;
    let mut end = 0;
    for (i, ch) in s.char_indices() {
        if matches!(ch, '<' | ';' | '.') {
            end = i;
            break;
        }
    }
    if end == 0 {
        return None;
    }
    let mut name = format_internal_qualified(&s[..end]);
    s = &s[end..];
    let mut args = Vec::new();
    if s.starts_with('<') {
        let (parsed_args, rest) = parse_type_args(s)?;
        args = parsed_args;
        s = rest;
    }
    while let Some(mut inner_s) = s.strip_prefix('.') {
        let mut inner_end = 0;
        for (i, ch) in inner_s.char_indices() {
            if matches!(ch, '<' | ';' | '.') {
                inner_end = i;
                break;
            }
        }
        if inner_end == 0 {
            return None;
        }
        name.push('.');
        name.push_str(&inner_s[..inner_end].replace('$', "."));
        inner_s = &inner_s[inner_end..];
        if inner_s.starts_with('<') {
            let (parsed_args, rest) = parse_type_args(inner_s)?;
            args = parsed_args;
            inner_s = rest;
        } else {
            args.clear();
        }
        s = inner_s;
    }
    s = s.strip_prefix(';')?;
    if !args.is_empty() {
        name = format!("{name}<{}>", args.join(","));
    }
    Some((name, s))
}

fn parse_type_sig(mut s: &str) -> Option<(String, &str)> {
    let mut array_dim = 0usize;
    while let Some(rest) = s.strip_prefix('[') {
        array_dim += 1;
        s = rest;
    }
    let (mut base, rest) = match s.as_bytes().first().copied()? {
        b'B' => ("byte".to_string(), &s[1..]),
        b'C' => ("char".to_string(), &s[1..]),
        b'D' => ("double".to_string(), &s[1..]),
        b'F' => ("float".to_string(), &s[1..]),
        b'I' => ("int".to_string(), &s[1..]),
        b'J' => ("long".to_string(), &s[1..]),
        b'S' => ("short".to_string(), &s[1..]),
        b'Z' => ("boolean".to_string(), &s[1..]),
        b'V' => ("void".to_string(), &s[1..]),
        b'T' => {
            let semi = s.find(';')?;
            (s[1..semi].to_string(), &s[semi + 1..])
        }
        b'L' => parse_class_type_sig(s)?,
        _ => return None,
    };
    for _ in 0..array_dim {
        base.push_str("[]");
    }
    Some((base, rest))
}

fn skip_formal_type_params(mut s: &str) -> Option<(Vec<String>, &str)> {
    if !s.starts_with('<') {
        return Some((Vec::new(), s));
    }
    s = &s[1..];
    let mut generics = Vec::new();
    while !s.starts_with('>') {
        let colon = s.find(':')?;
        generics.push(s[..colon].to_string());
        s = &s[colon..];
        while let Some(rest) = s.strip_prefix(':') {
            if rest.starts_with(':') {
                s = rest;
                continue;
            }
            let (_, next) = parse_type_sig(rest)?;
            s = next;
        }
    }
    Some((generics, &s[1..]))
}

fn parse_class_signature(sig: &str) -> Option<(Vec<String>, Option<String>, Vec<String>)> {
    let (generics, mut rest) = skip_formal_type_params(sig)?;
    let (super_sig, next) = parse_class_type_sig(rest)?;
    rest = next;
    let mut interfaces = Vec::new();
    while !rest.is_empty() {
        let (iface, next) = parse_class_type_sig(rest)?;
        interfaces.push(iface);
        rest = next;
    }
    Some((generics, Some(super_sig), interfaces))
}

fn parse_method_sig_return(sig: &str) -> Option<String> {
    let (_, rest) = skip_formal_type_params(sig)?;
    let mut s = rest.strip_prefix('(')?;
    while !s.starts_with(')') {
        let (_, next) = parse_type_sig(s)?;
        s = next;
    }
    let (ret, _) = parse_type_sig(&s[1..])?;
    Some(ret)
}

fn parse_method_param_count(desc: &str) -> Option<usize> {
    let mut s = desc.strip_prefix('(')?;
    let mut count = 0usize;
    while !s.starts_with(')') {
        let (_, next) = parse_type_sig(s)?;
        count += 1;
        s = next;
    }
    Some(count)
}

enum ElementValue {
    Scalar(String),
    Nested(Annotation),
    Array(Vec<ElementValue>),
}

fn parse_element_value(r: &mut ClassReader<'_>, cp: &[CpEntry]) -> Option<ElementValue> {
    let tag = r.u8()?;
    match tag {
        b's' => {
            let idx = r.u16()?;
            Some(ElementValue::Scalar(cp_utf8(cp, idx)?.to_string()))
        }
        b'B' | b'I' | b'S' => {
            let idx = r.u16()?;
            match cp.get(idx as usize)? {
                CpEntry::Int(v) => Some(ElementValue::Scalar(v.to_string())),
                _ => None,
            }
        }
        b'C' => {
            let idx = r.u16()?;
            match cp.get(idx as usize)? {
                CpEntry::Int(v) => Some(ElementValue::Scalar(
                    char::from_u32(*v as u32)
                        .map(|c| c.to_string())
                        .unwrap_or_else(|| v.to_string()),
                )),
                _ => None,
            }
        }
        b'Z' => {
            let idx = r.u16()?;
            match cp.get(idx as usize)? {
                CpEntry::Int(v) => Some(ElementValue::Scalar(
                    if *v != 0 { "true" } else { "false" }.to_string(),
                )),
                _ => None,
            }
        }
        b'J' => {
            let idx = r.u16()?;
            match cp.get(idx as usize)? {
                CpEntry::Long(v) => Some(ElementValue::Scalar(v.to_string())),
                _ => None,
            }
        }
        b'F' => {
            let idx = r.u16()?;
            match cp.get(idx as usize)? {
                CpEntry::Float(v) => Some(ElementValue::Scalar(v.to_string())),
                _ => None,
            }
        }
        b'D' => {
            let idx = r.u16()?;
            match cp.get(idx as usize)? {
                CpEntry::Double(v) => Some(ElementValue::Scalar(v.to_string())),
                _ => None,
            }
        }
        b'e' => {
            let type_idx = r.u16()?;
            let const_idx = r.u16()?;
            let type_desc = cp_utf8(cp, type_idx)?;
            let const_name = cp_utf8(cp, const_idx)?;
            let (ty, _) = parse_type_sig(type_desc)?;
            let simple = ty.rsplit('.').next().unwrap_or(&ty);
            Some(ElementValue::Scalar(format!("{simple}.{const_name}")))
        }
        b'c' => {
            let idx = r.u16()?;
            let desc = cp_utf8(cp, idx)?;
            let (ty, _) = parse_type_sig(desc)?;
            Some(ElementValue::Scalar(ty))
        }
        b'@' => parse_annotation_entry(r, cp).map(ElementValue::Nested),
        b'[' => {
            let num = r.u16()? as usize;
            let mut items = Vec::with_capacity(num);
            for _ in 0..num {
                items.push(parse_element_value(r, cp)?);
            }
            Some(ElementValue::Array(items))
        }
        _ => None,
    }
}

fn push_element_value(ann: &mut Annotation, key: &str, val: ElementValue) {
    match val {
        ElementValue::Scalar(s) => {
            ann.args.entry(key.to_string()).or_default().push(s);
        }
        ElementValue::Nested(nested) => {
            ann.nested.entry(key.to_string()).or_default().push(nested);
        }
        ElementValue::Array(items) => {
            for item in items {
                push_element_value(ann, key, item);
            }
        }
    }
}

fn parse_annotation_entry(r: &mut ClassReader<'_>, cp: &[CpEntry]) -> Option<Annotation> {
    let type_idx = r.u16()?;
    let type_desc = cp_utf8(cp, type_idx)?;
    let (full_name, _) = parse_type_sig(type_desc)?;
    let name = full_name
        .rsplit('.')
        .next()
        .unwrap_or(&full_name)
        .to_string();
    let mut ann = Annotation {
        name,
        full_name,
        args: BTreeMap::new(),
        nested: BTreeMap::new(),
        expressions: BTreeSet::new(),
        item_exprs: BTreeMap::new(),
    };
    let num_pairs = r.u16()? as usize;
    for _ in 0..num_pairs {
        let name_idx = r.u16()?;
        let key = cp_utf8(cp, name_idx)?.to_string();
        let val = parse_element_value(r, cp)?;
        push_element_value(&mut ann, &key, val);
    }
    Some(ann)
}

fn parse_annotations_attr(attr_bytes: &[u8], cp: &[CpEntry]) -> Option<Vec<Annotation>> {
    let mut r = ClassReader::new(attr_bytes);
    let count = r.u16()? as usize;
    let mut out = Vec::with_capacity(count);
    for _ in 0..count {
        out.push(parse_annotation_entry(&mut r, cp)?);
    }
    Some(out)
}

fn skip_jdk_base(name: &str) -> bool {
    let base = name.split('<').next().unwrap_or(name).trim();
    let simple = base.rsplit('.').next().unwrap_or(base);
    matches!(
        simple,
        "Object"
            | "Enum"
            | "Record"
            | "Serializable"
            | "Comparable"
            | "Cloneable"
            | "AutoCloseable"
    )
}

fn decapitalize(s: &str) -> String {
    let mut chars = s.chars();
    let Some(first) = chars.next() else {
        return String::new();
    };
    if let Some(second) = chars.next()
        && first.is_ascii_uppercase()
        && second.is_ascii_uppercase()
    {
        return s.to_string();
    }
    let mut out = first.to_lowercase().to_string();
    out.push_str(&s[first.len_utf8()..]);
    out
}

fn getter_property_name(method_name: &str, return_ty: &str) -> Option<String> {
    if method_name == "getClass" {
        return None;
    }
    let simple = return_ty.split('<').next().unwrap_or(return_ty).trim();
    if matches!(simple, "void" | "Void") {
        return None;
    }
    if let Some(rest) = method_name.strip_prefix("get")
        && rest.chars().next().is_some_and(|c| c.is_ascii_uppercase())
    {
        return Some(decapitalize(rest));
    }
    if let Some(rest) = method_name.strip_prefix("is")
        && rest.chars().next().is_some_and(|c| c.is_ascii_uppercase())
        && matches!(simple, "boolean" | "Boolean" | "java.lang.Boolean")
    {
        return Some(decapitalize(rest));
    }
    None
}

fn is_ignored_member(anns: &[Annotation]) -> bool {
    annotation(anns, "JsonIgnore").is_some_and(|a| a.value("value").as_deref() != Some("false"))
        || annotation(anns, "JSONField")
            .and_then(|a| a.value("serialize"))
            .as_deref()
            == Some("false")
        || annotation(anns, "Schema")
            .and_then(|a| a.value("hidden"))
            .as_deref()
            == Some("true")
}

/// 为从 `-sources.jar` 解析出的 Java 接口整理父接口继承并从 getter 推导字段属性。
pub fn synthesize_interface_fields(class: &mut Class) {
    if !class.is_interface {
        return;
    }
    for iface in class.interfaces.drain(..) {
        if !skip_jdk_base(&iface) && !class.bases.contains(&iface) {
            class.bases.push(iface);
        }
    }
    let mut existing: HashSet<String> = class.fields.iter().map(|f| f.name.clone()).collect();
    for m in &class.methods {
        if !m.params.is_empty() || is_ignored_member(&m.annotations) {
            continue;
        }
        let Some(prop) = getter_property_name(&m.name, &m.ty) else {
            continue;
        };
        if existing.insert(prop.clone()) {
            class.fields.push(Field {
                name: prop,
                ty: m.ty.clone(),
                annotations: m.annotations.clone(),
                doc: m.doc.clone(),
                is_final: false,
            });
        }
    }
}

pub fn parse_classfile(bytes: &[u8], source_label: &str) -> Option<Class> {
    let mut r = ClassReader::new(bytes);
    if r.u32()? != 0xCAFEBABE {
        return None;
    }
    let _minor = r.u16()?;
    let _major = r.u16()?;
    let cp_count = r.u16()? as usize;
    if cp_count == 0 {
        return None;
    }
    let mut cp = Vec::with_capacity(cp_count);
    cp.push(CpEntry::Placeholder);
    let mut idx = 1usize;
    while idx < cp_count {
        let tag = r.u8()?;
        let entry = match tag {
            1 => {
                let len = r.u16()? as usize;
                let raw = r.bytes(len)?;
                CpEntry::Utf8(decode_mutf8(raw))
            }
            3 => CpEntry::Int(r.i32()?),
            4 => CpEntry::Float(r.f32()?),
            5 => {
                let v = r.i64()?;
                cp.push(CpEntry::Long(v));
                cp.push(CpEntry::Placeholder);
                idx += 2;
                continue;
            }
            6 => {
                let v = r.f64()?;
                cp.push(CpEntry::Double(v));
                cp.push(CpEntry::Placeholder);
                idx += 2;
                continue;
            }
            7 => CpEntry::Class(r.u16()?),
            8 | 16 | 19 | 20 => {
                r.skip(2)?;
                CpEntry::Other
            }
            9 | 10 | 11 | 12 | 17 | 18 => {
                r.skip(4)?;
                CpEntry::Other
            }
            15 => {
                r.skip(3)?;
                CpEntry::Other
            }
            _ => return None,
        };
        cp.push(entry);
        idx += 1;
    }

    let access_flags = r.u16()?;
    let this_class_idx = r.u16()?;
    let super_class_idx = r.u16()?;
    let this_internal = cp_class_name(&cp, this_class_idx)?;
    let (package, name) = split_internal_name(this_internal);
    let is_interface = (access_flags & 0x0200) != 0;
    let is_enum = (access_flags & 0x4000) != 0;

    let mut super_name = if super_class_idx != 0 {
        cp_class_name(&cp, super_class_idx).map(format_internal_qualified)
    } else {
        None
    };

    let iface_count = r.u16()? as usize;
    let mut iface_names = Vec::with_capacity(iface_count);
    for _ in 0..iface_count {
        let idx = r.u16()?;
        if let Some(internal) = cp_class_name(&cp, idx) {
            iface_names.push(format_internal_qualified(internal));
        }
    }

    let field_count = r.u16()? as usize;
    let mut fields = Vec::new();
    let mut enum_variants = Vec::new();
    let mut enum_items = Vec::new();
    for _ in 0..field_count {
        let f_flags = r.u16()?;
        let f_name = cp_utf8(&cp, r.u16()?)?.to_string();
        let f_desc = cp_utf8(&cp, r.u16()?)?;
        let attr_count = r.u16()? as usize;
        let mut f_sig = None;
        let mut f_anns = Vec::new();
        for _ in 0..attr_count {
            let attr_name = cp_utf8(&cp, r.u16()?)?;
            let attr_len = r.u32()? as usize;
            let attr_bytes = r.bytes(attr_len)?;
            match attr_name {
                "Signature" if attr_bytes.len() == 2 => {
                    let s_idx = u16::from_be_bytes([attr_bytes[0], attr_bytes[1]]);
                    f_sig = cp_utf8(&cp, s_idx).map(str::to_string);
                }
                "RuntimeVisibleAnnotations" => {
                    if let Some(parsed) = parse_annotations_attr(attr_bytes, &cp) {
                        f_anns.extend(parsed);
                    }
                }
                _ => (),
            }
        }
        let is_static = (f_flags & 0x0008) != 0;
        let is_transient = (f_flags & 0x0080) != 0;
        let is_synthetic = (f_flags & 0x1000) != 0;
        let is_enum_const = (f_flags & 0x4000) != 0;
        if is_enum && is_static && is_enum_const {
            enum_variants.push(f_name.clone());
            enum_items.push(EnumVariant {
                name: f_name,
                annotations: f_anns,
                ..Default::default()
            });
            continue;
        }
        if is_static || is_transient || is_synthetic {
            continue;
        }
        let ty = f_sig
            .as_deref()
            .and_then(|s| parse_type_sig(s).map(|(t, _)| t))
            .or_else(|| parse_type_sig(f_desc).map(|(t, _)| t))?;
        fields.push(Field {
            name: f_name,
            ty,
            annotations: f_anns,
            doc: String::new(),
            is_final: false,
        });
    }

    struct GetterInfo {
        prop: String,
        ty: String,
        anns: Vec<Annotation>,
    }
    let mut getters = Vec::new();
    let method_count = r.u16()? as usize;
    for _ in 0..method_count {
        let m_flags = r.u16()?;
        let m_name = cp_utf8(&cp, r.u16()?)?;
        let m_desc = cp_utf8(&cp, r.u16()?)?;
        let attr_count = r.u16()? as usize;
        let mut m_sig = None;
        let mut m_anns = Vec::new();
        for _ in 0..attr_count {
            let attr_name = cp_utf8(&cp, r.u16()?)?;
            let attr_len = r.u32()? as usize;
            let attr_bytes = r.bytes(attr_len)?;
            match attr_name {
                "Signature" if attr_bytes.len() == 2 => {
                    let s_idx = u16::from_be_bytes([attr_bytes[0], attr_bytes[1]]);
                    m_sig = cp_utf8(&cp, s_idx).map(str::to_string);
                }
                "RuntimeVisibleAnnotations" => {
                    if let Some(parsed) = parse_annotations_attr(attr_bytes, &cp) {
                        m_anns.extend(parsed);
                    }
                }
                _ => (),
            }
        }
        let is_public = (m_flags & 0x0001) != 0;
        let is_static = (m_flags & 0x0008) != 0;
        let is_bridge = (m_flags & 0x0040) != 0;
        let is_synthetic = (m_flags & 0x1000) != 0;
        if !is_public || is_static || is_bridge || is_synthetic {
            continue;
        }
        if parse_method_param_count(m_desc) != Some(0) {
            continue;
        }
        let return_ty = m_sig
            .as_deref()
            .and_then(parse_method_sig_return)
            .or_else(|| parse_method_sig_return(m_desc));
        let Some(return_ty) = return_ty else {
            continue;
        };
        if let Some(prop) = getter_property_name(m_name, &return_ty) {
            getters.push(GetterInfo {
                prop,
                ty: return_ty,
                anns: m_anns,
            });
        }
    }

    let mut class_anns = Vec::new();
    let mut class_sig = None;
    let class_attr_count = r.u16()? as usize;
    for _ in 0..class_attr_count {
        let attr_name = cp_utf8(&cp, r.u16()?)?;
        let attr_len = r.u32()? as usize;
        let attr_bytes = r.bytes(attr_len)?;
        match attr_name {
            "Signature" if attr_bytes.len() == 2 => {
                let s_idx = u16::from_be_bytes([attr_bytes[0], attr_bytes[1]]);
                class_sig = cp_utf8(&cp, s_idx).map(str::to_string);
            }
            "RuntimeVisibleAnnotations" => {
                if let Some(parsed) = parse_annotations_attr(attr_bytes, &cp) {
                    class_anns.extend(parsed);
                }
            }
            _ => (),
        }
    }

    let mut generics = Vec::new();
    if let Some((g, sup, ifaces)) = class_sig.as_deref().and_then(parse_class_signature) {
        generics = g;
        if let Some(s) = sup {
            super_name = Some(s);
        }
        if !ifaces.is_empty() {
            iface_names = ifaces;
        }
    }

    let mut bases = Vec::new();
    let mut interfaces = Vec::new();
    if is_interface {
        for iface in iface_names {
            if !skip_jdk_base(&iface) {
                bases.push(iface);
            }
        }
    } else {
        if let Some(sup) = super_name
            && !skip_jdk_base(&sup)
        {
            bases.push(sup);
        }
        interfaces = iface_names;
    }

    if !is_enum {
        let no_instance_fields = fields.is_empty();
        let mut field_index: HashMap<String, usize> = fields
            .iter()
            .enumerate()
            .map(|(i, f)| (f.name.clone(), i))
            .collect();
        for g in getters {
            if let Some(&idx) = field_index.get(&g.prop) {
                for ann in g.anns {
                    if !fields[idx].annotations.iter().any(|a| a.name == ann.name) {
                        fields[idx].annotations.push(ann);
                    }
                }
            } else if (is_interface
                || no_instance_fields
                || annotation(&g.anns, "JsonProperty").is_some())
                && !is_ignored_member(&g.anns)
            {
                field_index.insert(g.prop.clone(), fields.len());
                fields.push(Field {
                    name: g.prop,
                    ty: g.ty,
                    annotations: g.anns,
                    doc: String::new(),
                    is_final: false,
                });
            }
        }
    }

    Some(Class {
        name,
        package,
        path: source_label.to_string(),
        annotations: class_anns,
        generics,
        bases,
        interfaces,
        fields,
        variants: enum_variants,
        enum_items,
        is_interface,
        is_external: true,
        ..Default::default()
    })
}

/// 解析 `javap -p -v` 的批量输出，提取类/接口/枚举的泛型签名、字段、getter 与 RuntimeVisibleAnnotations。
pub fn parse_javap_verbose(output: &str) -> Vec<Class> {
    let mut blocks: Vec<String> = Vec::new();
    let mut current = String::new();
    for line in output.lines() {
        if line.starts_with("Classfile ") && !current.is_empty() {
            blocks.push(std::mem::take(&mut current));
        }
        current.push_str(line);
        current.push('\n');
    }
    if !current.trim().is_empty() {
        blocks.push(current);
    }
    blocks
        .iter()
        .filter_map(|block| parse_javap_block(block))
        .collect()
}

fn parse_javap_block(block: &str) -> Option<Class> {
    let lines: Vec<&str> = block.lines().collect();
    let mut jar_name: Option<String> = None;
    let mut this_internal: Option<String> = None;
    let mut super_internal: Option<String> = None;
    let mut header_decl: Option<&str> = None;
    let mut class_flags = String::new();
    let mut class_sig: Option<String> = None;
    let mut class_anns: Vec<Annotation> = Vec::new();
    let mut body_start: Option<usize> = None;
    let mut body_end: Option<usize> = None;

    for (i, &line) in lines.iter().enumerate() {
        let trimmed = line.trim();
        if let Some(rest) = line.strip_prefix("Classfile ") {
            if let Some((before_bang, _)) = rest.split_once("!/") {
                jar_name = before_bang.rsplit(['/', '\\']).next().map(str::to_string);
            }
            continue;
        }
        if body_start.is_none() {
            if trimmed == "{" {
                body_start = Some(i);
                continue;
            }
            if header_decl.is_none()
                && !line.starts_with(' ')
                && (trimmed.contains(" class ")
                    || trimmed.contains(" interface ")
                    || trimmed.contains(" enum ")
                    || trimmed.starts_with("class ")
                    || trimmed.starts_with("interface ")
                    || trimmed.starts_with("enum "))
            {
                header_decl = Some(trimmed);
            } else if let Some(rest) = trimmed.strip_prefix("flags:") {
                class_flags = rest.to_string();
            } else if trimmed.starts_with("this_class:")
                && let Some((_, comment)) = trimmed.split_once("//")
            {
                this_internal = Some(comment.trim().to_string());
            } else if trimmed.starts_with("super_class:")
                && let Some((_, comment)) = trimmed.split_once("//")
            {
                super_internal = Some(comment.trim().to_string());
            }
        } else if body_end.is_none() && line == "}" {
            body_end = Some(i);
        }
    }

    let is_interface = class_flags.contains("ACC_INTERFACE")
        || header_decl.is_some_and(|d| d.contains("interface "));
    let is_enum =
        class_flags.contains("ACC_ENUM") || header_decl.is_some_and(|d| d.contains(" enum "));

    let (package, name) = if let Some(internal) = this_internal {
        split_internal_name(&internal)
    } else {
        let decl = header_decl?;
        let kw = if is_interface {
            "interface "
        } else if is_enum {
            "enum "
        } else {
            "class "
        };
        let after = decl.split_once(kw)?.1;
        let raw_name = after
            .split(|c: char| c.is_whitespace() || c == '<' || c == '{')
            .next()?
            .trim();
        if let Some((p, c)) = raw_name.rsplit_once('.') {
            (p.to_string(), c.to_string())
        } else {
            (String::new(), raw_name.to_string())
        }
    };

    let qualified = if package.is_empty() {
        name.clone()
    } else {
        format!("{package}.{name}")
    };
    let source_label = match jar_name {
        Some(jar) => format!("{jar} ({qualified})"),
        None => format!("javap ({qualified})"),
    };

    // 解析类级属性（在 "}" 之后）
    if let Some(end_idx) = body_end {
        let mut i = end_idx + 1;
        while i < lines.len() {
            let trimmed = lines[i].trim();
            if trimmed.starts_with("Signature:")
                && let Some((_, comment)) = trimmed.split_once("//")
            {
                class_sig = Some(comment.trim().to_string());
                i += 1;
            } else if trimmed == "RuntimeVisibleAnnotations:" {
                let (anns, next_i) = parse_javap_annotations(&lines, i + 1, 4);
                class_anns.extend(anns);
                i = next_i;
            } else {
                i += 1;
            }
        }
    }

    let mut generics = Vec::new();
    let mut bases = Vec::new();
    let mut interfaces = Vec::new();
    if let Some((g, sup, ifaces)) = class_sig.as_deref().and_then(parse_class_signature) {
        generics = g;
        if is_interface {
            for iface in ifaces {
                if !skip_jdk_base(&iface) {
                    bases.push(iface);
                }
            }
        } else {
            if let Some(s) = sup
                && !skip_jdk_base(&s)
            {
                bases.push(s);
            }
            interfaces = ifaces;
        }
    } else if !is_interface
        && let Some(sup) = super_internal.as_deref().map(format_internal_qualified)
        && !skip_jdk_base(&sup)
    {
        bases.push(sup);
    }

    let mut fields = Vec::new();
    let mut enum_variants = Vec::new();
    let mut enum_items = Vec::new();
    struct GetterInfo {
        prop: String,
        ty: String,
        anns: Vec<Annotation>,
    }
    let mut getters = Vec::new();

    if let (Some(start), Some(end)) = (body_start, body_end) {
        let mut i = start + 1;
        while i < end {
            let line = lines[i];
            if line.starts_with("  ") && !line.starts_with("   ") && line.trim_end().ends_with(';')
            {
                let decl = line.trim();
                let is_method = decl.contains('(');
                let mut desc = None;
                let mut sig = None;
                let mut flags = String::new();
                let mut anns = Vec::new();
                let mut j = i + 1;
                while j < end {
                    let sub = lines[j];
                    if sub.starts_with("  ")
                        && !sub.starts_with("   ")
                        && sub.trim_end().ends_with(';')
                    {
                        break;
                    }
                    let sub_trim = sub.trim();
                    if let Some(rest) = sub_trim.strip_prefix("descriptor:") {
                        desc = Some(rest.trim().to_string());
                    } else if let Some(rest) = sub_trim.strip_prefix("flags:") {
                        flags = rest.to_string();
                    } else if sub_trim.starts_with("Signature:")
                        && let Some((_, comment)) = sub_trim.split_once("//")
                    {
                        sig = Some(comment.trim().to_string());
                    } else if sub_trim == "RuntimeVisibleAnnotations:" {
                        let (parsed, next_j) = parse_javap_annotations(&lines, j + 1, 6);
                        anns.extend(parsed);
                        j = next_j;
                        continue;
                    }
                    j += 1;
                }

                if !is_method {
                    let fname = decl
                        .trim_end_matches(';')
                        .split_whitespace()
                        .last()
                        .unwrap_or("")
                        .to_string();
                    let is_static = flags.contains("ACC_STATIC");
                    let is_transient = flags.contains("ACC_TRANSIENT");
                    let is_synthetic = flags.contains("ACC_SYNTHETIC");
                    let is_enum_const = flags.contains("ACC_ENUM");
                    if is_enum && is_static && is_enum_const && !fname.is_empty() {
                        enum_variants.push(fname.clone());
                        enum_items.push(EnumVariant {
                            name: fname,
                            annotations: anns,
                            ..Default::default()
                        });
                    } else if !is_static && !is_transient && !is_synthetic && !fname.is_empty() {
                        let ty = sig
                            .as_deref()
                            .and_then(|s| parse_type_sig(s).map(|(t, _)| t))
                            .or_else(|| {
                                desc.as_deref()
                                    .and_then(|d| parse_type_sig(d).map(|(t, _)| t))
                            });
                        if let Some(ty) = ty {
                            fields.push(Field {
                                name: fname,
                                ty,
                                annotations: anns,
                                doc: String::new(),
                                is_final: false,
                            });
                        }
                    }
                } else {
                    let is_public = flags.contains("ACC_PUBLIC");
                    let is_static = flags.contains("ACC_STATIC");
                    let is_bridge = flags.contains("ACC_BRIDGE");
                    let is_synthetic = flags.contains("ACC_SYNTHETIC");
                    if is_public
                        && !is_static
                        && !is_bridge
                        && !is_synthetic
                        && let Some(d) = desc.as_deref()
                        && parse_method_param_count(d) == Some(0)
                    {
                        let mname = decl
                            .split('(')
                            .next()
                            .and_then(|head| head.split_whitespace().last())
                            .unwrap_or("");
                        let return_ty = sig
                            .as_deref()
                            .and_then(parse_method_sig_return)
                            .or_else(|| parse_method_sig_return(d));
                        if let Some(return_ty) = return_ty
                            && let Some(prop) = getter_property_name(mname, &return_ty)
                        {
                            getters.push(GetterInfo {
                                prop,
                                ty: return_ty,
                                anns,
                            });
                        }
                    }
                }
                i = j;
                continue;
            }
            i += 1;
        }
    }

    if !is_enum {
        let no_instance_fields = fields.is_empty();
        let mut field_index: HashMap<String, usize> = fields
            .iter()
            .enumerate()
            .map(|(i, f)| (f.name.clone(), i))
            .collect();
        for g in getters {
            if let Some(&idx) = field_index.get(&g.prop) {
                for ann in g.anns {
                    if !fields[idx].annotations.iter().any(|a| a.name == ann.name) {
                        fields[idx].annotations.push(ann);
                    }
                }
            } else if (is_interface
                || no_instance_fields
                || annotation(&g.anns, "JsonProperty").is_some())
                && !is_ignored_member(&g.anns)
            {
                field_index.insert(g.prop.clone(), fields.len());
                fields.push(Field {
                    name: g.prop,
                    ty: g.ty,
                    annotations: g.anns,
                    doc: String::new(),
                    is_final: false,
                });
            }
        }
    }

    Some(Class {
        name,
        package,
        path: source_label,
        annotations: class_anns,
        generics,
        bases,
        interfaces,
        fields,
        variants: enum_variants,
        enum_items,
        is_interface,
        is_external: true,
        ..Default::default()
    })
}

fn parse_javap_annotations(
    lines: &[&str],
    mut idx: usize,
    entry_indent: usize,
) -> (Vec<Annotation>, usize) {
    let mut out = Vec::new();
    while idx < lines.len() {
        let line = lines[idx];
        let indent = line.len() - line.trim_start().len();
        if indent < entry_indent {
            break;
        }
        let trimmed = line.trim();
        // javap 在每个注解前先输出一行 "0: #71(#72=s#73)"，下一行才是解码后的注解名
        if trimmed
            .split_once(':')
            .is_some_and(|(num, _)| !num.is_empty() && num.chars().all(|c| c.is_ascii_digit()))
        {
            idx += 1;
            continue;
        }
        if indent == entry_indent + 2 {
            if let Some(ann_name) = trimmed.strip_suffix('(') {
                let mut ann = Annotation {
                    name: ann_name.rsplit('.').next().unwrap_or(ann_name).to_string(),
                    full_name: ann_name.to_string(),
                    ..Default::default()
                };
                idx += 1;
                while idx < lines.len() {
                    let sub = lines[idx];
                    let sub_indent = sub.len() - sub.trim_start().len();
                    let sub_trim = sub.trim();
                    if sub_indent == entry_indent + 2 && sub_trim == ")" {
                        idx += 1;
                        break;
                    }
                    if sub_indent < entry_indent + 2 {
                        break;
                    }
                    if let Some((k, v)) = sub_trim.split_once('=') {
                        push_javap_ann_val(&mut ann, k.trim(), v.trim());
                    }
                    idx += 1;
                }
                out.push(ann);
                continue;
            } else if !trimmed.is_empty() && !trimmed.starts_with(')') {
                let ann_name = trimmed.trim_end_matches("()");
                out.push(Annotation {
                    name: ann_name.rsplit('.').next().unwrap_or(ann_name).to_string(),
                    full_name: ann_name.to_string(),
                    ..Default::default()
                });
                idx += 1;
                continue;
            }
        }
        idx += 1;
    }
    (out, idx)
}

fn push_javap_ann_val(ann: &mut Annotation, key: &str, raw: &str) {
    let raw = raw.trim_end_matches(',');
    if (raw.starts_with('[') && raw.ends_with(']')) || (raw.starts_with('{') && raw.ends_with('}'))
    {
        let inner = &raw[1..raw.len() - 1];
        for item in split_javap_array(inner) {
            push_javap_ann_val(ann, key, item.trim());
        }
        return;
    }
    let val = if raw.starts_with('"') && raw.ends_with('"') && raw.len() >= 2 {
        serde_json::from_str::<String>(raw).unwrap_or_else(|_| raw[1..raw.len() - 1].to_string())
    } else if let Some((enum_desc, const_name)) = raw.split_once(";.") {
        let internal = enum_desc.trim_start_matches('L');
        let full = format_internal_qualified(internal);
        let simple = full.rsplit('.').next().unwrap_or(&full);
        format!("{simple}.{const_name}")
    } else {
        raw.to_string()
    };
    ann.args.entry(key.to_string()).or_default().push(val);
}

fn split_javap_array(s: &str) -> Vec<&str> {
    let mut out = Vec::new();
    let mut start = 0;
    let mut in_quotes = false;
    let mut escaped = false;
    for (idx, ch) in s.char_indices() {
        if escaped {
            escaped = false;
            continue;
        }
        match ch {
            '\\' if in_quotes => escaped = true,
            '"' => in_quotes = !in_quotes,
            ',' if !in_quotes => {
                let part = s[start..idx].trim();
                if !part.is_empty() {
                    out.push(part);
                }
                start = idx + 1;
            }
            _ => (),
        }
    }
    let tail = s[start..].trim();
    if !tail.is_empty() {
        out.push(tail);
    }
    out
}
