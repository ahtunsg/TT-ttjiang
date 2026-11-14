//! Embeds `assets/app.ico` into the executable as a Win32 icon resource.
//!
//! Until 4.0.4 the brand mark only existed at runtime: `tray.rs` pulls the ICO
//! bytes with `include_bytes!` and turns them into an `HICON` with
//! `CreateIconFromResourceEx`. That icon shows in the taskbar and in the search
//! box, but Explorer, the Alt+Tab list and any shortcut read the icon from the
//! PE resource directory — which a plain `rustc` link never creates, so the file
//! showed up with the generic blank icon.
//!
//! No resource compiler is guaranteed to exist on a build machine (`windres`,
//! `rc.exe` and `llvm-rc` are all absent here), so this script writes the COFF
//! object itself: one `.rsrc` section holding an RT_GROUP_ICON plus one
//! RT_ICON per ICO frame. Every `IMAGE_RESOURCE_DATA_ENTRY.OffsetToData` is a
//! section-relative offset paired with an `IMAGE_REL_AMD64_ADDR32NB` relocation,
//! which is exactly what windres emits; GNU ld then fills in the real RVAs and
//! points the PE resource data directory at the section.

use std::fs;
use std::path::PathBuf;

const ICO: &[u8] = include_bytes!("assets/app.ico");

const RT_ICON: u32 = 3;
const RT_GROUP_ICON: u32 = 14;

struct Frame {
    w: u8,
    h: u8,
    colors: u8,
    planes: u16,
    bits: u16,
    data: Vec<u8>,
}

fn u16_at(b: &[u8], o: usize) -> u16 {
    u16::from_le_bytes([b[o], b[o + 1]])
}

fn u32_at(b: &[u8], o: usize) -> u32 {
    u32::from_le_bytes([b[o], b[o + 1], b[o + 2], b[o + 3]])
}

fn put16(b: &mut [u8], o: usize, v: u16) {
    b[o..o + 2].copy_from_slice(&v.to_le_bytes());
}

fn put32(b: &mut [u8], o: usize, v: u32) {
    b[o..o + 4].copy_from_slice(&v.to_le_bytes());
}

/// Split the ICO container into its frames.
fn parse_ico(ico: &[u8]) -> Result<Vec<Frame>, String> {
    if ico.len() < 6 || u16_at(ico, 0) != 0 || u16_at(ico, 2) != 1 {
        return Err("不是 ICO 格式".into());
    }
    let count = u16_at(ico, 4) as usize;
    if count == 0 || 6 + count * 16 > ico.len() {
        return Err("ICO 目录长度异常".into());
    }
    let mut frames = Vec::with_capacity(count);
    for i in 0..count {
        let e = 6 + i * 16;
        let size = u32_at(ico, e + 8) as usize;
        let off = u32_at(ico, e + 12) as usize;
        if off + size > ico.len() {
            return Err(format!("第 {} 帧越界", i));
        }
        frames.push(Frame {
            w: ico[e],
            h: ico[e + 1],
            colors: ico[e + 2],
            planes: u16_at(ico, e + 4),
            bits: u16_at(ico, e + 6),
            data: ico[off..off + size].to_vec(),
        });
    }
    Ok(frames)
}

/// GRPICONDIR: the ICO header plus one 14-byte entry per frame (`nID` instead
/// of the ICO's 4-byte `dwImageOffset`, which is where a naive port drifts).
fn group_icon(frames: &[Frame]) -> Vec<u8> {
    let mut g = Vec::with_capacity(6 + frames.len() * 14);
    g.extend_from_slice(&0u16.to_le_bytes()); // reserved
    g.extend_from_slice(&1u16.to_le_bytes()); // type: icon
    g.extend_from_slice(&(frames.len() as u16).to_le_bytes());
    for (i, f) in frames.iter().enumerate() {
        g.push(f.w);
        g.push(f.h);
        g.push(f.colors);
        g.push(0);
        g.extend_from_slice(&f.planes.to_le_bytes());
        g.extend_from_slice(&f.bits.to_le_bytes());
        g.extend_from_slice(&(f.data.len() as u32).to_le_bytes());
        g.extend_from_slice(&((i + 1) as u16).to_le_bytes()); // resource id
    }
    g
}

/// Builds the `.rsrc` blobs and returns them plus the offsets of the fields
/// that need an ADDR32NB relocation.
fn build_rsrc(ico: &[u8]) -> Result<(Vec<u8>, Vec<usize>), String> {
    let frames = parse_ico(ico)?;
    let n = frames.len();
    let grp = group_icon(&frames);

    // Directory layout, all offsets relative to the section start:
    // root -> type dirs -> per-id dirs -> language 0 dir -> data entry.
    let root_sz = 16 + 8 * 2;
    let icon_sz = 16 + 8 * n;
    let grp_sz = 16 + 8;
    let leaf_sz = 16 + 8;

    let off_root = 0usize;
    let off_icon = off_root + root_sz;
    let off_grp = off_icon + icon_sz;
    let off_leaf0 = off_grp + grp_sz;
    let leaf: Vec<usize> = (0..n + 1).map(|i| off_leaf0 + i * leaf_sz).collect();
    let off_entries = leaf[n] + leaf_sz;

    let blobs: Vec<&[u8]> = frames.iter().map(|f| f.data.as_slice()).chain(std::iter::once(grp.as_slice())).collect();
    let entry_sz = 16;
    let mut offsets = Vec::with_capacity(blobs.len());
    let mut cur = off_entries + entry_sz * blobs.len();
    for b in &blobs {
        cur = (cur + 3) & !3;
        offsets.push(cur);
        cur += b.len();
    }

    let mut buf = vec![0u8; cur];
    let mut relocs = Vec::with_capacity(blobs.len());

    // header + entries of one directory level; `target` already has its flag bit.
    let dir = |buf: &mut [u8], off: usize, entries: &[(u32, u32)]| {
        put16(buf, off + 12, 0); // named entries
        put16(buf, off + 14, entries.len() as u16);
        for (i, (name, target)) in entries.iter().enumerate() {
            put32(buf, off + 16 + i * 8, *name);
            put32(buf, off + 16 + i * 8 + 4, *target);
        }
    };

    let dirbit = 0x8000_0000u32;
    dir(&mut buf, off_root, &[(RT_ICON, dirbit | off_icon as u32), (RT_GROUP_ICON, dirbit | off_grp as u32)]);
    let icon_entries: Vec<(u32, u32)> = (0..n).map(|i| ((i + 1) as u32, dirbit | leaf[i] as u32)).collect();
    dir(&mut buf, off_icon, &icon_entries);
    dir(&mut buf, off_grp, &[(1, dirbit | leaf[n] as u32)]);
    for i in 0..n + 1 {
        dir(&mut buf, leaf[i], &[(0, (off_entries + i * entry_sz) as u32)]);
    }

    for (i, b) in blobs.iter().enumerate() {
        let pos = off_entries + i * entry_sz;
        put32(&mut buf, pos, offsets[i] as u32); // addend for the ADDR32NB reloc
        put32(&mut buf, pos + 4, b.len() as u32);
        put32(&mut buf, pos + 8, 0); // code page
        put32(&mut buf, pos + 12, 0);
        relocs.push(pos);
        buf[offsets[i]..offsets[i] + b.len()].copy_from_slice(b);
    }
    Ok((buf, relocs))
}

/// One-section AMD64 COFF object: header, `.rsrc` data, relocations, symbol.
fn build_coff(rsrc: &[u8], relocs: &[usize]) -> Vec<u8> {
    let hdr = 20 + 40;
    let raw_ptr = hdr;
    let rel_ptr = raw_ptr + rsrc.len();
    let sym_ptr = rel_ptr + relocs.len() * 10;
    let mut out = Vec::with_capacity(sym_ptr + 18 + 4);

    // IMAGE_FILE_HEADER
    out.extend_from_slice(&0x8664u16.to_le_bytes()); // AMD64
    out.extend_from_slice(&1u16.to_le_bytes()); // one section
    out.extend_from_slice(&0u32.to_le_bytes()); // timestamp
    out.extend_from_slice(&(sym_ptr as u32).to_le_bytes());
    out.extend_from_slice(&1u32.to_le_bytes()); // symbols
    out.extend_from_slice(&0u16.to_le_bytes()); // no optional header
    out.extend_from_slice(&0u16.to_le_bytes());

    // IMAGE_SECTION_HEADER for `.rsrc`
    out.extend_from_slice(b".rsrc\0\0\0");
    out.extend_from_slice(&(rsrc.len() as u32).to_le_bytes()); // VirtualSize
    out.extend_from_slice(&0u32.to_le_bytes()); // VirtualAddress
    out.extend_from_slice(&(rsrc.len() as u32).to_le_bytes()); // SizeOfRawData
    out.extend_from_slice(&(raw_ptr as u32).to_le_bytes());
    out.extend_from_slice(&(rel_ptr as u32).to_le_bytes());
    out.extend_from_slice(&0u32.to_le_bytes()); // no line numbers
    out.extend_from_slice(&(relocs.len() as u16).to_le_bytes());
    out.extend_from_slice(&0u16.to_le_bytes());
    out.extend_from_slice(&0x4000_0040u32.to_le_bytes()); // INITIALIZED_DATA | READ

    out.extend_from_slice(rsrc);

    for r in relocs {
        out.extend_from_slice(&(*r as u32).to_le_bytes());
        out.extend_from_slice(&0u32.to_le_bytes()); // symbol 0: the `.rsrc` section
        out.extend_from_slice(&0x0003u16.to_le_bytes()); // IMAGE_REL_AMD64_ADDR32NB
    }

    // Symbol table: section symbol for `.rsrc`, static class.
    out.extend_from_slice(b".rsrc\0\0\0");
    out.extend_from_slice(&0u32.to_le_bytes()); // value
    out.extend_from_slice(&1i16.to_le_bytes()); // section number
    out.extend_from_slice(&0u16.to_le_bytes()); // type
    out.push(3); // IMAGE_SYM_CLASS_STATIC
    out.push(0); // no aux records
    out.extend_from_slice(&0u32.to_le_bytes()); // empty string table
    out
}

fn main() {
    println!("cargo:rerun-if-changed=assets/app.ico");
    let (rsrc, relocs) = match build_rsrc(ICO) {
        Ok(v) => v,
        Err(e) => {
            println!("cargo:warning=图标资源未内嵌（{}），exe 在资源管理器里会显示空白图标", e);
            return;
        }
    };
    let out_dir = PathBuf::from(std::env::var("OUT_DIR").expect("OUT_DIR"));
    let obj = out_dir.join("app_icon.o");
    if let Err(e) = fs::write(&obj, build_coff(&rsrc, &relocs)) {
        println!("cargo:warning=图标资源写入失败: {}", e);
        return;
    }
    println!("cargo:rustc-link-arg={}", obj.display());
}
