use std::fs;
use std::path::PathBuf;

const RT_ICON: u16 = 3;
const RT_GROUP_ICON: u16 = 14;

fn main() {
    println!("cargo:rerun-if-changed=build.rs");
    // GPUI's native layout/paint traversal with the embedded editor exceeds
    // MSVC's default 1 MiB main-thread reserve in debug builds. Reserve 8 MiB
    // for the GUI executable; Windows commits pages only as they are touched.
    if std::env::var("CARGO_CFG_TARGET_OS").as_deref() == Ok("windows")
        && std::env::var("CARGO_CFG_TARGET_ENV").as_deref() == Ok("msvc")
    {
        println!("cargo:rustc-link-arg-bin=studyapp=/STACK:8388608");
        embed_app_icon();
    }
}

/// Embed `assets/icon.ico` as the executable's icon resource group 1.
///
/// GPUI's Windows backend loads its window class icon with
/// `LoadImageW(module, MAKEINTRESOURCE(1), IMAGE_ICON, ...)`, so the group must
/// keep id 1 and reference every image of the source icon.
fn embed_app_icon() {
    let manifest = PathBuf::from(std::env::var("CARGO_MANIFEST_DIR").unwrap());
    let icon = manifest.join("../../assets/icon.ico");
    println!("cargo:rerun-if-changed={}", icon.display());
    let data = fs::read(&icon).unwrap_or_else(|error| {
        panic!(
            "cannot read the application icon {}: {error}",
            icon.display()
        )
    });
    let resources = icon_resources(&data)
        .unwrap_or_else(|error| panic!("invalid application icon {}: {error}", icon.display()));
    let out = PathBuf::from(std::env::var("OUT_DIR").unwrap()).join("studyapp-icon.res");
    fs::write(&out, resource_file(&resources)).expect("cannot write the icon resource file");
    println!("cargo:rustc-link-arg-bin=studyapp={}", out.display());
}

struct Image {
    width: u8,
    height: u8,
    color_count: u8,
    planes: u16,
    bit_count: u16,
    data: Vec<u8>,
}

fn icon_resources(bytes: &[u8]) -> Result<Vec<(u16, u16, Vec<u8>)>, String> {
    if bytes.len() < 6 {
        return Err("header is too short".into());
    }
    let reserved = u16::from_le_bytes([bytes[0], bytes[1]]);
    let kind = u16::from_le_bytes([bytes[2], bytes[3]]);
    let count = u16::from_le_bytes([bytes[4], bytes[5]]) as usize;
    if reserved != 0 || kind != 1 || count == 0 || count > 64 {
        return Err(format!(
            "unsupported icon header ({reserved}/{kind}/{count})"
        ));
    }
    let mut images = Vec::with_capacity(count);
    for index in 0..count {
        let at = 6 + index * 16;
        if bytes.len() < at + 16 {
            return Err("truncated directory".into());
        }
        let entry = &bytes[at..at + 16];
        let size = u32::from_le_bytes([entry[8], entry[9], entry[10], entry[11]]) as usize;
        let offset = u32::from_le_bytes([entry[12], entry[13], entry[14], entry[15]]) as usize;
        if size == 0 || offset.checked_add(size).is_none_or(|end| end > bytes.len()) {
            return Err(format!("image {index} is outside the file"));
        }
        images.push(Image {
            width: entry[0],
            height: entry[1],
            color_count: entry[2],
            planes: u16::from_le_bytes([entry[4], entry[5]]),
            bit_count: u16::from_le_bytes([entry[6], entry[7]]),
            data: bytes[offset..offset + size].to_vec(),
        });
    }

    let mut resources = Vec::with_capacity(count + 1);
    for (position, image) in images.iter().enumerate() {
        resources.push((RT_ICON, position as u16 + 1, image.data.clone()));
    }

    let mut group = Vec::with_capacity(6 + count * 14);
    group.extend_from_slice(&0u16.to_le_bytes());
    group.extend_from_slice(&1u16.to_le_bytes());
    group.extend_from_slice(&(count as u16).to_le_bytes());
    for (position, image) in images.iter().enumerate() {
        group.push(image.width);
        group.push(image.height);
        group.push(image.color_count);
        group.push(0);
        group.extend_from_slice(&image.planes.to_le_bytes());
        group.extend_from_slice(&image.bit_count.to_le_bytes());
        group.extend_from_slice(&(image.data.len() as u32).to_le_bytes());
        group.extend_from_slice(&(position as u16 + 1).to_le_bytes());
    }
    resources.push((RT_GROUP_ICON, 1, group));
    Ok(resources)
}

fn resource_file(resources: &[(u16, u16, Vec<u8>)]) -> Vec<u8> {
    // The file starts with a null entry whose HeaderSize is 0x20; CVTRES rejects
    // the file if that field is left at zero.
    let mut out = vec![0u8; 32];
    out[4..8].copy_from_slice(&32u32.to_le_bytes());
    for (kind, name, data) in resources {
        let header_size: u32 = 8 + 4 + 4 + 4 + 2 + 2 + 4 + 4;
        out.extend_from_slice(&(data.len() as u32).to_le_bytes());
        out.extend_from_slice(&header_size.to_le_bytes());
        out.extend_from_slice(&0xFFFFu16.to_le_bytes());
        out.extend_from_slice(&kind.to_le_bytes());
        out.extend_from_slice(&0xFFFFu16.to_le_bytes());
        out.extend_from_slice(&name.to_le_bytes());
        out.extend_from_slice(&0u32.to_le_bytes());
        out.extend_from_slice(&0x1030u16.to_le_bytes());
        out.extend_from_slice(&0x0409u16.to_le_bytes());
        out.extend_from_slice(&0u32.to_le_bytes());
        out.extend_from_slice(&0u32.to_le_bytes());
        out.extend_from_slice(data);
        while !out.len().is_multiple_of(4) {
            out.push(0);
        }
    }
    out
}
