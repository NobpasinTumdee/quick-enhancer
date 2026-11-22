//! Embeds the Windows icon and version information into QuickEnhancer.exe.
//! The version comes from Cargo.toml (`CARGO_PKG_VERSION`) — the single source of truth.
//!
//! No resource compiler (rc.exe / windres) is needed: this script writes the COFF object with a
//! `.rsrc` section itself (what `cvtres` / `windres` would produce) and links it into the binary.
//! Both the GNU (ld) and MSVC (link.exe) linkers accept it.

use std::path::Path;

const RT_ICON: u32 = 3;
const RT_GROUP_ICON: u32 = 14;
const RT_VERSION: u32 = 16;
const LANG_EN_US: u32 = 0x0409;

fn main() {
    println!("cargo:rerun-if-changed=assets/icon.ico");
    println!("cargo:rerun-if-changed=build.rs");
    if std::env::var("CARGO_CFG_TARGET_OS").as_deref() != Ok("windows") {
        return;
    }
    let arch = std::env::var("CARGO_CFG_TARGET_ARCH").unwrap_or_default();
    let machine: u16 = match arch.as_str() {
        "x86_64" => 0x8664,
        "aarch64" => 0xAA64,
        "x86" => 0x014C,
        _ => {
            println!("cargo:warning=no Windows resources for target arch {arch}");
            return;
        }
    };
    let version = std::env::var("CARGO_PKG_VERSION").unwrap_or_default();
    let ico = match std::fs::read("assets/icon.ico") {
        Ok(b) => b,
        Err(e) => {
            println!("cargo:warning=assets/icon.ico not readable ({e}); exe has no icon");
            return;
        }
    };
    let mut res: Vec<(u32, u32, Vec<u8>)> = Vec::new();
    match icon_resources(&ico) {
        Some((icons, group)) => {
            for (i, data) in icons.into_iter().enumerate() {
                res.push((RT_ICON, i as u32 + 1, data));
            }
            res.push((RT_GROUP_ICON, 1, group));
        }
        None => println!("cargo:warning=assets/icon.ico is not a valid .ico; exe has no icon"),
    }
    res.push((RT_VERSION, 1, version_info(&version)));

    let out = Path::new(&std::env::var("OUT_DIR").expect("OUT_DIR")).join("quick-enhancer-res.o");
    std::fs::write(&out, coff_object(machine, &res)).expect("write resource object");
    println!("cargo:rustc-link-arg-bins={}", out.display());
}

/// Split a .ico file into RT_ICON payloads + the RT_GROUP_ICON directory.
fn icon_resources(ico: &[u8]) -> Option<(Vec<Vec<u8>>, Vec<u8>)> {
    let u16_at = |o: usize| Some(u16::from_le_bytes(ico.get(o..o + 2)?.try_into().ok()?));
    let u32_at = |o: usize| Some(u32::from_le_bytes(ico.get(o..o + 4)?.try_into().ok()?));
    if u16_at(0)? != 0 || u16_at(2)? != 1 {
        return None;
    }
    let n = u16_at(4)? as usize;
    let mut icons = Vec::new();
    let mut group = Vec::new();
    group.extend(0u16.to_le_bytes());
    group.extend(1u16.to_le_bytes());
    group.extend((n as u16).to_le_bytes());
    for i in 0..n {
        let e = 6 + i * 16;
        let size = u32_at(e + 8)? as usize;
        let offset = u32_at(e + 12)? as usize;
        icons.push(ico.get(offset..offset + size)?.to_vec());
        // GRPICONDIRENTRY: the first 12 bytes of ICONDIRENTRY + the resource id.
        group.extend(ico.get(e..e + 12)?);
        group.extend((i as u16 + 1).to_le_bytes());
    }
    Some((icons, group))
}

fn utf16z(s: &str) -> Vec<u8> {
    s.encode_utf16().chain(std::iter::once(0)).flat_map(|c| c.to_le_bytes()).collect()
}

fn pad4(v: &mut Vec<u8>) {
    while !v.len().is_multiple_of(4) {
        v.push(0);
    }
}

/// One VERSIONINFO block: wLength, wValueLength, wType, key, padding, value, children.
fn block(key: &str, wtype: u16, value: &[u8], value_len: u16, children: &[Vec<u8>]) -> Vec<u8> {
    let mut v = vec![0, 0];
    v.extend(value_len.to_le_bytes());
    v.extend(wtype.to_le_bytes());
    v.extend(utf16z(key));
    pad4(&mut v);
    v.extend(value);
    for c in children {
        pad4(&mut v);
        v.extend(c);
    }
    let len = v.len() as u16;
    v[0..2].copy_from_slice(&len.to_le_bytes());
    v
}

fn version_info(version: &str) -> Vec<u8> {
    let mut p = version.split(['.', '-', '+']).map(|x| x.parse::<u32>().unwrap_or(0));
    let (a, b, c) = (p.next().unwrap_or(0), p.next().unwrap_or(0), p.next().unwrap_or(0));
    let ms = (a << 16) | (b & 0xFFFF);
    let ls = (c & 0xFFFF) << 16;
    let mut fixed = Vec::new();
    for x in [0xFEEF04BDu32, 0x0001_0000, ms, ls, ms, ls, 0x3F, 0, 0x0004_0004, 1, 0, 0, 0] {
        fixed.extend(x.to_le_bytes());
    }
    let strings = [
        ("CompanyName", "NobpasinTumdee"),
        ("FileDescription", "Quick Enhancer — local image & video enhancement"),
        ("FileVersion", version),
        ("InternalName", "QuickEnhancer"),
        ("LegalCopyright", "Copyright (c) Quick Enhancer contributors"),
        ("OriginalFilename", "QuickEnhancer.exe"),
        ("ProductName", "Quick Enhancer"),
        ("ProductVersion", version),
    ];
    let children: Vec<Vec<u8>> = strings
        .iter()
        .map(|(k, v)| {
            let val = utf16z(v);
            block(k, 1, &val, (val.len() / 2) as u16, &[])
        })
        .collect();
    let table = block("040904b0", 1, &[], 0, &children);
    let sfi = block("StringFileInfo", 1, &[], 0, &[table]);
    let mut tr = Vec::new();
    tr.extend(0x0409u16.to_le_bytes());
    tr.extend(0x04B0u16.to_le_bytes());
    let var = block("Translation", 0, &tr, 4, &[]);
    let vfi = block("VarFileInfo", 1, &[], 0, &[var]);
    block("VS_VERSION_INFO", 0, &fixed, fixed.len() as u16, &[sfi, vfi])
}

/// COFF object with one `.rsrc` section: type → id → language directory tree, data entries
/// (their RVAs relocated against the section symbol) and the data blobs.
fn coff_object(machine: u16, res: &[(u32, u32, Vec<u8>)]) -> Vec<u8> {
    let mut types: Vec<u32> = res.iter().map(|r| r.0).collect();
    types.sort();
    types.dedup();
    let dir = |n: usize| -> Vec<u8> {
        let mut v = vec![0u8; 12];
        v.extend(0u16.to_le_bytes());
        v.extend((n as u16).to_le_bytes());
        v
    };
    let n_types = types.len();
    let n_res = res.len();
    // Layout: root dir, type dirs (each with its id entries), language dirs (1 entry each),
    // data entries, blobs.
    let root_size = 16 + 8 * n_types;
    let type_dirs_size: usize = types.iter().map(|t| 16 + 8 * res.iter().filter(|r| r.0 == *t).count()).sum();
    let lang_dirs_size = n_res * (16 + 8);
    let data_entries_at = root_size + type_dirs_size + lang_dirs_size;
    let blobs_at = data_entries_at + n_res * 16;

    // Resources in tree order (type, then id).
    let mut order: Vec<usize> = (0..n_res).collect();
    order.sort_by_key(|&i| (res[i].0, res[i].1));

    let mut sec = dir(n_types);
    let mut type_off = root_size;
    for t in &types {
        sec.extend(t.to_le_bytes());
        sec.extend((0x8000_0000u32 | type_off as u32).to_le_bytes());
        type_off += 16 + 8 * res.iter().filter(|r| r.0 == *t).count();
    }
    let lang_dirs_at = root_size + type_dirs_size;
    let mut k = 0usize;
    for t in &types {
        let ids: Vec<usize> = order.iter().copied().filter(|&i| res[i].0 == *t).collect();
        sec.extend(dir(ids.len()));
        // `order` is sorted by (type, id), so the k-th entry here is `order[k]`.
        for &i in &ids {
            sec.extend(res[i].1.to_le_bytes());
            sec.extend((0x8000_0000u32 | (lang_dirs_at + k * 24) as u32).to_le_bytes());
            k += 1;
        }
    }
    for k in 0..n_res {
        sec.extend(dir(1));
        sec.extend(LANG_EN_US.to_le_bytes());
        sec.extend(((data_entries_at + k * 16) as u32).to_le_bytes());
    }
    assert_eq!(sec.len(), data_entries_at);
    let mut blob_off = blobs_at;
    let mut relocs = Vec::new();
    for &i in &order {
        relocs.push(sec.len() as u32);
        sec.extend((blob_off as u32).to_le_bytes());
        sec.extend((res[i].2.len() as u32).to_le_bytes());
        sec.extend(0u32.to_le_bytes());
        sec.extend(0u32.to_le_bytes());
        blob_off += res[i].2.len().div_ceil(8) * 8;
    }
    for &i in &order {
        sec.extend(&res[i].2);
        while !sec.len().is_multiple_of(8) {
            sec.push(0);
        }
    }

    let reloc_type: u16 = match machine {
        0x8664 => 3,  // IMAGE_REL_AMD64_ADDR32NB
        0xAA64 => 2,  // IMAGE_REL_ARM64_ADDR32NB
        _ => 7,       // IMAGE_REL_I386_DIR32NB
    };
    let header = 20;
    let sec_hdr = 40;
    let raw_at = header + sec_hdr;
    let reloc_at = raw_at + sec.len();
    let sym_at = reloc_at + relocs.len() * 10;
    let mut o = Vec::new();
    o.extend(machine.to_le_bytes());
    o.extend(1u16.to_le_bytes());
    o.extend(0u32.to_le_bytes());
    o.extend((sym_at as u32).to_le_bytes());
    o.extend(1u32.to_le_bytes());
    o.extend(0u16.to_le_bytes());
    o.extend(0u16.to_le_bytes());
    o.extend(*b".rsrc\0\0\0");
    o.extend(0u32.to_le_bytes());
    o.extend(0u32.to_le_bytes());
    o.extend((sec.len() as u32).to_le_bytes());
    o.extend((raw_at as u32).to_le_bytes());
    o.extend((reloc_at as u32).to_le_bytes());
    o.extend(0u32.to_le_bytes());
    o.extend((relocs.len() as u16).to_le_bytes());
    o.extend(0u16.to_le_bytes());
    // INITIALIZED_DATA | ALIGN_4BYTES | MEM_READ
    o.extend((0x0000_0040u32 | 0x0030_0000 | 0x4000_0000).to_le_bytes());
    o.extend(&sec);
    for r in relocs {
        o.extend(r.to_le_bytes());
        o.extend(0u32.to_le_bytes()); // symbol 0 = .rsrc section symbol
        o.extend(reloc_type.to_le_bytes());
    }
    // Symbol: .rsrc, value 0, section 1, type 0, static, no aux.
    o.extend(*b".rsrc\0\0\0");
    o.extend(0u32.to_le_bytes());
    o.extend(1i16.to_le_bytes());
    o.extend(0u16.to_le_bytes());
    o.push(3);
    o.push(0);
    // Empty string table.
    o.extend(4u32.to_le_bytes());
    o
}
