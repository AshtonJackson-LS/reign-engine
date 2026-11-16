//! Putting a project's icon in the game Export Game makes.
//!
//! The window icon (see `icon.rs`) only shows once the game is running. The
//! icon of the game's *file* (in Explorer, on the Desktop, in the taskbar's
//! pinned buttons, in Finder and the Dock) lives in the file itself:
//! - **Windows**: icon resources in the `.exe`. [`set_exe_icon`] writes them
//!   with the system's resource-update calls, into the copy Export makes
//!   (never the build output, and never a running program).
//! - **macOS**: an `.icns` file in the app's `Resources`, named in
//!   `Info.plist` ([`icns`]).
//! - **Linux** has no icon inside an executable; the exported folder holds
//!   `icon.png` for a launcher to point at.
//!
//! The game's name, version and author (Project Settings) go in the Windows
//! program the same way, as its version resource ([`set_exe_version`]), which
//! Explorer shows under Properties → Details. macOS has them in `Info.plist`.

use reign::assets::RgbaImage;

use crate::icon;

/// The sizes put in a Windows icon, the ones Explorer, the taskbar and
/// the title bar ask for, smallest to largest. Only sizes the icon has the
/// pixels for are used (a 64 px icon gets 16 to 64).
#[cfg(any(windows, test))]
pub const WINDOWS_SIZES: [u32; 7] = [16, 24, 32, 48, 64, 128, 256];

/// One size of an icon, as PNG (which Windows has taken in icons since
/// Vista).
#[cfg(any(windows, test))]
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct IconEntry {
    pub size: u32,
    pub png: Vec<u8>,
}

/// `image` at each of [`WINDOWS_SIZES`] it can fill.
#[cfg(any(windows, test))]
pub fn windows_entries(image: &RgbaImage) -> Vec<IconEntry> {
    WINDOWS_SIZES
        .into_iter()
        .filter(|&size| size <= image.width.min(image.height))
        .map(|size| IconEntry { size, png: icon::encode_png(&icon::resize(image, size)) })
        .collect()
}

/// The resource that groups the sizes: Windows' `GRPICONDIR`, a header and
/// one 14-byte entry per size naming the resource (1, 2, 3...) that holds
/// its pixels.
#[cfg(test)]
pub fn group_icon(entries: &[IconEntry]) -> Vec<u8> {
    group_icon_from(entries, 1)
}

/// [`group_icon`] with the sizes in resources `first`, `first + 1`...
#[cfg(any(windows, test))]
fn group_icon_from(entries: &[IconEntry], first: u16) -> Vec<u8> {
    let mut out = Vec::with_capacity(6 + entries.len() * 14);
    out.extend_from_slice(&0u16.to_le_bytes()); // reserved
    out.extend_from_slice(&1u16.to_le_bytes()); // 1 = icon
    out.extend_from_slice(&(entries.len() as u16).to_le_bytes());
    for (i, entry) in entries.iter().enumerate() {
        // 256 is written as 0: the field is one byte.
        let side = if entry.size >= 256 { 0 } else { entry.size as u8 };
        out.push(side); // width
        out.push(side); // height
        out.push(0); // colours in the palette: none
        out.push(0); // reserved
        out.extend_from_slice(&1u16.to_le_bytes()); // colour planes
        out.extend_from_slice(&32u16.to_le_bytes()); // bits per pixel
        out.extend_from_slice(&(entry.png.len() as u32).to_le_bytes());
        out.extend_from_slice(&(first + i as u16).to_le_bytes()); // resource id
    }
    out
}

/// An `.icns` file for a macOS app: PNG data for 128, 256, 512 and 1024
/// pixels, the ones the icon is big enough for (128 and 256 always: a small
/// icon is enlarged to them).
pub fn icns(image: &RgbaImage) -> Vec<u8> {
    let side = image.width.min(image.height);
    let mut chunks: Vec<u8> = Vec::new();
    for (kind, size) in [(b"ic07", 128), (b"ic08", 256), (b"ic09", 512), (b"ic10", 1024)] {
        if size > 256 && size > side {
            continue;
        }
        let png = icon::encode_png(&icon::resize(image, size));
        chunks.extend_from_slice(kind);
        chunks.extend_from_slice(&(png.len() as u32 + 8).to_be_bytes());
        chunks.extend_from_slice(&png);
    }
    let mut out = Vec::with_capacity(chunks.len() + 8);
    out.extend_from_slice(b"icns");
    out.extend_from_slice(&(chunks.len() as u32 + 8).to_be_bytes());
    out.extend_from_slice(&chunks);
    out
}

/// Whether the Windows program `exe` already has an icon (a project can
/// embed its own when it is built), under any id or name. Always false
/// elsewhere, where there is none to have.
pub fn has_exe_icon(exe: &std::path::Path) -> bool {
    #[cfg(windows)]
    {
        windows::icon_count(exe) > 0
    }
    #[cfg(not(windows))]
    {
        let _ = exe;
        false
    }
}

/// Writes `image` as the icon of the Windows program `exe`, keeping its
/// other resources. The file must not be running. Other systems keep no
/// icon in the program, so there it does nothing.
pub fn set_exe_icon(exe: &std::path::Path, image: &RgbaImage) -> Result<(), String> {
    #[cfg(windows)]
    {
        windows::update(exe, &windows_entries(image))
    }
    #[cfg(not(windows))]
    {
        let _ = (exe, image);
        Ok(())
    }
}

/// What goes in a Windows program's version resource.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct VersionInfo {
    /// The version as four numbers (major, minor, patch, build).
    pub numbers: [u16; 4],
    /// The version as written in Project Settings, such as `1.0.0`.
    pub version: String,
    /// The game's name.
    pub title: String,
    /// Who made it; empty leaves the company and copyright out.
    pub author: String,
    /// The program's file name, such as `space_rocks.exe`.
    pub exe_name: String,
    /// The crate's name.
    pub internal_name: String,
}

/// The language and code page the strings are under: US English, Unicode.
#[cfg(any(windows, test))]
const VERSION_LANGUAGE: u16 = 0x0409;
#[cfg(any(windows, test))]
const VERSION_CODEPAGE: u16 = 0x04B0;

/// Windows' `VS_VERSIONINFO`: the fixed numbers, then the strings Explorer
/// shows, then the language they're in. Every block is its length, its
/// value's length, whether the value is text, a UTF-16 key, padding to four
/// bytes, the value, and its children, each on a four-byte boundary.
#[cfg(any(windows, test))]
pub fn version_resource(info: &VersionInfo) -> Vec<u8> {
    let [a, b, c, d] = info.numbers.map(u32::from);
    let (ms, ls) = ((a << 16) | b, (c << 16) | d);
    let fixed_words = [
        0xFEEF_04BD, // signature
        0x0001_0000, // structure version 1.0
        ms,          // file version
        ls,
        ms, // product version
        ls,
        0x3F,        // the flags that are meaningful: all of them
        0,           // flags: a release build
        0x0004_0004, // for Windows NT, 32-bit API (which 64-bit programs use too)
        1,           // a program
        0,
        0, // no date
        0,
    ];
    let fixed: Vec<u8> = fixed_words.into_iter().flat_map(u32::to_le_bytes).collect();

    let author = info.author.trim();
    let mut strings: Vec<(&str, String)> = Vec::new();
    if !author.is_empty() {
        strings.push(("CompanyName", author.to_owned()));
    }
    strings.push(("FileDescription", info.title.clone()));
    strings.push(("FileVersion", info.version.clone()));
    strings.push(("InternalName", info.internal_name.clone()));
    if !author.is_empty() {
        strings.push(("LegalCopyright", format!("© {author}")));
    }
    strings.push(("OriginalFilename", info.exe_name.clone()));
    strings.push(("ProductName", info.title.clone()));
    strings.push(("ProductVersion", info.version.clone()));
    let table: Vec<Vec<u8>> = strings
        .iter()
        .map(|(key, value)| {
            let text = utf16(value);
            version_block(key, true, &text, (text.len() / 2) as u16, &[])
        })
        .collect();
    let table_key = format!("{VERSION_LANGUAGE:04X}{VERSION_CODEPAGE:04X}");
    let table = version_block(&table_key, true, &[], 0, &table);
    let string_info = version_block("StringFileInfo", true, &[], 0, &[table]);
    let mut translation = VERSION_LANGUAGE.to_le_bytes().to_vec();
    translation.extend_from_slice(&VERSION_CODEPAGE.to_le_bytes());
    let var = version_block("Translation", false, &translation, 4, &[]);
    let var_info = version_block("VarFileInfo", true, &[], 0, &[var]);
    version_block("VS_VERSION_INFO", false, &fixed, 52, &[string_info, var_info])
}

/// `text` as NUL-terminated UTF-16 bytes.
#[cfg(any(windows, test))]
fn utf16(text: &str) -> Vec<u8> {
    text.encode_utf16().chain(Some(0)).flat_map(u16::to_le_bytes).collect()
}

/// One block of a version resource (see [`version_resource`]).
/// `value_length` is in bytes for binary values and in UTF-16 units for
/// text, as Windows counts them.
#[cfg(any(windows, test))]
fn version_block(
    key: &str,
    is_text: bool,
    value: &[u8],
    value_length: u16,
    children: &[Vec<u8>],
) -> Vec<u8> {
    let pad = |out: &mut Vec<u8>| {
        while !out.len().is_multiple_of(4) {
            out.push(0);
        }
    };
    let mut out = vec![0, 0]; // the length, filled in at the end
    out.extend_from_slice(&value_length.to_le_bytes());
    out.extend_from_slice(&u16::from(is_text).to_le_bytes());
    out.extend_from_slice(&utf16(key));
    pad(&mut out);
    out.extend_from_slice(value);
    for child in children {
        pad(&mut out);
        out.extend_from_slice(child);
    }
    let length = out.len() as u16;
    out[..2].copy_from_slice(&length.to_le_bytes());
    out
}

/// Writes the game's name, version and author into the Windows program
/// `exe`, keeping its other resources (an earlier version resource is
/// replaced). The file must not be running. Elsewhere it does nothing.
pub fn set_exe_version(exe: &std::path::Path, info: &VersionInfo) -> Result<(), String> {
    #[cfg(windows)]
    {
        windows::update_version(exe, &version_resource(info))
    }
    #[cfg(not(windows))]
    {
        let _ = (exe, info);
        Ok(())
    }
}

/// Test helpers for real programs: see `windows::probe`.
#[cfg(all(windows, test))]
pub(crate) use windows::probe;

#[cfg(windows)]
mod windows {
    //! The Windows API calls behind the icon: the three resource-update calls
    //! (the way to change a program's resources; there is no safe equivalent
    //! in the standard library), and the shell's icon count. They are only
    //! given pointers to data this module owns for the length of each call.

    use std::ffi::c_void;
    use std::os::windows::ffi::OsStrExt;
    use std::path::Path;

    use super::{IconEntry, VERSION_LANGUAGE, group_icon_from};

    // Resource types, as the Windows API names them (`MAKEINTRESOURCE`).
    const RT_ICON: usize = 3;
    const RT_GROUP_ICON: usize = 14;
    const RT_VERSION: usize = 16;
    /// The version resource's number: there is one, and it's 1.
    const VERSION_ID: usize = 1;

    unsafe extern "system" {
        fn BeginUpdateResourceW(file_name: *const u16, delete_existing: i32) -> *mut c_void;
        fn UpdateResourceW(
            update: *mut c_void,
            kind: *const u16,
            name: *const u16,
            language: u16,
            data: *const c_void,
            size: u32,
        ) -> i32;
        fn EndUpdateResourceW(update: *mut c_void, discard: i32) -> i32;
    }

    #[link(name = "shell32")]
    unsafe extern "system" {
        fn ExtractIconExW(
            file: *const u16,
            index: i32,
            large: *mut *mut c_void,
            small: *mut *mut c_void,
            count: u32,
        ) -> u32;
    }

    fn wide(path: &Path) -> Vec<u16> {
        path.as_os_str().encode_wide().chain(Some(0)).collect()
    }

    /// How many icons the file holds, whatever their ids or names: the
    /// shell's own count (index -1 asks for it and loads nothing). 0 for a
    /// file that isn't a program, or isn't there.
    pub(super) fn icon_count(exe: &Path) -> u32 {
        let path = wide(exe);
        // SAFETY: `path` is NUL-terminated and outlives the call; with index
        // -1 and no output arrays the call only counts.
        let count = unsafe {
            ExtractIconExW(path.as_ptr(), -1, std::ptr::null_mut(), std::ptr::null_mut(), 0)
        };
        // Some versions return u32::MAX for a file they can't read.
        if count == u32::MAX { 0 } else { count }
    }

    /// Writes `entries` as icon group 1 (the one Explorer shows first among
    /// numbered icons) with icons 1, 2, 3...
    pub(super) fn update(exe: &Path, entries: &[IconEntry]) -> Result<(), String> {
        write(exe, entries, 1, 1, false)
    }

    /// Writes `resource` (a `VS_VERSIONINFO`) as the program's version
    /// resource, number 1 in US English, where Explorer looks.
    pub(super) fn update_version(exe: &Path, resource: &[u8]) -> Result<(), String> {
        let path = wide(exe);
        // SAFETY: `path` is NUL-terminated and outlives the call; the
        // program's other resources are kept.
        let handle = unsafe { BeginUpdateResourceW(path.as_ptr(), 0) };
        if handle.is_null() {
            return Err(format!("can't open {}: {}", exe.display(), os_error()));
        }
        // SAFETY: `handle` is the open update above; the type and name are
        // resource ids in pointer form, as the API takes them; `resource` is
        // valid for its length during the call.
        let ok = unsafe {
            UpdateResourceW(
                handle,
                RT_VERSION as *const u16,
                VERSION_ID as *const u16,
                VERSION_LANGUAGE,
                resource.as_ptr().cast(),
                resource.len() as u32,
            )
        };
        let result =
            if ok == 0 { Err(format!("can't write the version: {}", os_error())) } else { Ok(()) };
        // SAFETY: closes the update, dropping it if the write failed.
        let saved = unsafe { EndUpdateResourceW(handle, i32::from(result.is_err())) };
        if result.is_ok() && saved == 0 {
            return Err(format!("can't save the version into {}: {}", exe.display(), os_error()));
        }
        result
    }

    /// Writes an icon group: `group` names it, its sizes are icons
    /// `first_icon`, `first_icon + 1`... With `fresh`, every resource the
    /// program had is removed first (tests use it to make a program with no
    /// icon).
    fn write(
        exe: &Path,
        entries: &[IconEntry],
        group: usize,
        first_icon: usize,
        fresh: bool,
    ) -> Result<(), String> {
        if entries.is_empty() && !fresh {
            return Err("the icon has no sizes to write".into());
        }
        let path = wide(exe);
        // SAFETY: `path` is NUL-terminated and outlives the call. Unless
        // `fresh`, the program's other resources are kept.
        let handle = unsafe { BeginUpdateResourceW(path.as_ptr(), i32::from(fresh)) };
        if handle.is_null() {
            return Err(format!("can't open {} to set its icon: {}", exe.display(), os_error()));
        }
        let mut result = Ok(());
        let mut put = |kind: usize, id: usize, data: &[u8]| {
            // SAFETY: `handle` is the open update above; the type and name
            // are resource ids (small integers in pointer form, as the API
            // takes them); `data` is valid for `size` bytes during the call.
            let ok = unsafe {
                UpdateResourceW(
                    handle,
                    kind as *const u16,
                    id as *const u16,
                    0,
                    data.as_ptr().cast(),
                    data.len() as u32,
                )
            };
            if ok == 0 && result.is_ok() {
                result = Err(format!("can't write the icon: {}", os_error()));
            }
        };
        if !entries.is_empty() {
            for (i, entry) in entries.iter().enumerate() {
                put(RT_ICON, first_icon + i, &entry.png);
            }
            put(RT_GROUP_ICON, group, &group_icon_from(entries, first_icon as u16));
        }
        // SAFETY: closes the update; `discard` is true when something failed,
        // so a half-written icon is dropped rather than saved.
        let saved = unsafe { EndUpdateResourceW(handle, i32::from(result.is_err())) };
        if result.is_ok() && saved == 0 {
            result = Err(format!("can't save the icon into {}: {}", exe.display(), os_error()));
        }
        result
    }

    fn os_error() -> std::io::Error {
        std::io::Error::last_os_error()
    }

    #[cfg(test)]
    pub(crate) mod probe {
        //! For the tests: programs with no icon, or one under another id, and
        //! reading resources back.

        use super::*;

        unsafe extern "system" {
            fn LoadLibraryExW(file_name: *const u16, file: *mut c_void, flags: u32) -> *mut c_void;
            fn FreeLibrary(module: *mut c_void) -> i32;
            fn FindResourceW(
                module: *mut c_void,
                name: *const u16,
                kind: *const u16,
            ) -> *mut c_void;
            fn SizeofResource(module: *mut c_void, resource: *mut c_void) -> u32;
        }

        const LOAD_LIBRARY_AS_DATAFILE: u32 = 0x2;
        pub const ICON: usize = RT_ICON;
        pub const GROUP: usize = RT_GROUP_ICON;
        pub const VERSION: usize = RT_VERSION;

        /// Removes every resource from `exe` (a copy): a program with no icon.
        pub fn strip(exe: &Path) {
            write(exe, &[], 0, 0, true).expect("resources removed");
        }

        /// Gives `exe` an icon the way another tool might: group `group`,
        /// icons from `first_icon`.
        pub fn put_icon_as(exe: &Path, entries: &[IconEntry], group: usize, first_icon: usize) {
            write(exe, entries, group, first_icon, false).expect("icon written");
        }

        #[link(name = "version")]
        unsafe extern "system" {
            fn GetFileVersionInfoSizeW(file_name: *const u16, handle: *mut u32) -> u32;
            fn GetFileVersionInfoW(
                file_name: *const u16,
                handle: u32,
                len: u32,
                data: *mut c_void,
            ) -> i32;
            fn VerQueryValueW(
                block: *const c_void,
                sub_block: *const u16,
                buffer: *mut *mut c_void,
                len: *mut u32,
            ) -> i32;
        }

        /// The version string `key` (such as `ProductName`) of `exe`, read
        /// the way Explorer reads it.
        pub fn version_string(exe: &Path, key: &str) -> Option<String> {
            let path = wide(exe);
            let query: Vec<u16> = format!("\\StringFileInfo\\040904B0\\{key}")
                .encode_utf16()
                .chain(Some(0))
                .collect();
            // SAFETY: the buffer is as big as the system asked for and
            // outlives the queries; the value it points `found` at lies
            // inside the buffer and is copied out before the buffer goes.
            unsafe {
                let size = GetFileVersionInfoSizeW(path.as_ptr(), std::ptr::null_mut());
                if size == 0 {
                    return None;
                }
                let mut data = vec![0u8; size as usize];
                if GetFileVersionInfoW(path.as_ptr(), 0, size, data.as_mut_ptr().cast()) == 0 {
                    return None;
                }
                let mut found: *mut c_void = std::ptr::null_mut();
                let mut len = 0u32;
                if VerQueryValueW(data.as_ptr().cast(), query.as_ptr(), &mut found, &mut len) == 0
                    || len == 0
                {
                    return None;
                }
                let units = std::slice::from_raw_parts(found as *const u16, len as usize);
                let text = String::from_utf16_lossy(units);
                Some(text.trim_end_matches('\0').to_owned())
            }
        }

        /// The size in bytes of resource `id` of `kind` in `exe`, if it has one.
        pub fn resource_size(exe: &Path, kind: usize, id: usize) -> Option<u32> {
            let path = wide(exe);
            // SAFETY: the module is loaded as data only (nothing runs), freed
            // before returning, and the resource isn't used after that.
            unsafe {
                let module =
                    LoadLibraryExW(path.as_ptr(), std::ptr::null_mut(), LOAD_LIBRARY_AS_DATAFILE);
                if module.is_null() {
                    return None;
                }
                let found = FindResourceW(module, id as *const u16, kind as *const u16);
                let size = (!found.is_null()).then(|| SizeofResource(module, found));
                FreeLibrary(module);
                size
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn flat(side: u32, color: [u8; 4]) -> RgbaImage {
        RgbaImage {
            rgba: color.iter().copied().cycle().take((side * side * 4) as usize).collect(),
            width: side,
            height: side,
        }
    }

    #[test]
    fn windows_gets_the_sizes_the_icon_can_fill() {
        let sizes = |side| {
            windows_entries(&flat(side, [1, 2, 3, 255])).iter().map(|e| e.size).collect::<Vec<_>>()
        };
        assert_eq!(sizes(64), [16, 24, 32, 48, 64]);
        assert_eq!(sizes(100), [16, 24, 32, 48, 64]);
        assert_eq!(sizes(256), [16, 24, 32, 48, 64, 128, 256]);
        assert_eq!(sizes(1024), [16, 24, 32, 48, 64, 128, 256]);
        // Every entry is a PNG of its size.
        for entry in windows_entries(&flat(256, [9, 8, 7, 255])) {
            let image = icon::check_any_size(&entry.png).unwrap();
            assert_eq!((image.width, image.height), (entry.size, entry.size));
        }
    }

    #[test]
    fn the_group_resource_lists_every_size() {
        let entries = windows_entries(&flat(256, [9, 8, 7, 255]));
        let group = group_icon(&entries);
        assert_eq!(group.len(), 6 + 14 * entries.len());
        assert_eq!(&group[..6], &[0, 0, 1, 0, entries.len() as u8, 0]);
        for (i, entry) in entries.iter().enumerate() {
            let at = 6 + i * 14;
            let side = if entry.size == 256 { 0 } else { entry.size as u8 };
            assert_eq!(&group[at..at + 2], &[side, side], "size {}", entry.size);
            assert_eq!(&group[at + 4..at + 8], &[1, 0, 32, 0], "planes and bits");
            assert_eq!(
                u32::from_le_bytes(group[at + 8..at + 12].try_into().unwrap()),
                entry.png.len() as u32
            );
            assert_eq!(
                u16::from_le_bytes(group[at + 12..at + 14].try_into().unwrap()),
                i as u16 + 1
            );
        }
    }

    /// Reads an `.icns` back into (type, payload) chunks.
    fn icns_chunks(bytes: &[u8]) -> Vec<(String, Vec<u8>)> {
        assert_eq!(&bytes[..4], b"icns");
        assert_eq!(u32::from_be_bytes(bytes[4..8].try_into().unwrap()) as usize, bytes.len());
        let mut at = 8;
        let mut chunks = Vec::new();
        while at < bytes.len() {
            let kind = String::from_utf8(bytes[at..at + 4].to_vec()).unwrap();
            let len = u32::from_be_bytes(bytes[at + 4..at + 8].try_into().unwrap()) as usize;
            chunks.push((kind, bytes[at + 8..at + len].to_vec()));
            at += len;
        }
        assert_eq!(at, bytes.len(), "the chunks fill the file exactly");
        chunks
    }

    #[test]
    fn the_icns_has_the_sizes_the_icon_can_fill() {
        let kinds = |side| {
            icns_chunks(&icns(&flat(side, [5, 6, 7, 255])))
                .into_iter()
                .map(|c| c.0)
                .collect::<Vec<_>>()
        };
        assert_eq!(kinds(64), ["ic07", "ic08"], "small icons are enlarged to 128 and 256");
        assert_eq!(kinds(256), ["ic07", "ic08"]);
        assert_eq!(kinds(512), ["ic07", "ic08", "ic09"]);
        assert_eq!(kinds(1024), ["ic07", "ic08", "ic09", "ic10"]);
        for (kind, png) in icns_chunks(&icns(&flat(1024, [5, 6, 7, 255]))) {
            let want = match kind.as_str() {
                "ic07" => 128,
                "ic08" => 256,
                "ic09" => 512,
                _ => 1024,
            };
            let image = icon::check_any_size(&png).unwrap();
            assert_eq!((image.width, image.height), (want, want), "{kind}");
        }
    }

    #[test]
    fn a_program_without_an_icon_is_noticed() {
        let dir = std::env::temp_dir().join(format!("reign-exe-icon-has-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let fake = dir.join("fake.exe");
        std::fs::write(&fake, b"not really a game").unwrap();
        assert!(!has_exe_icon(&fake), "not a program: no icon");
        assert!(!has_exe_icon(&dir.join("missing.exe")));
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// A real program with every resource removed has no icon; given one
    /// under an id other than the usual (the way other tools may write it),
    /// it is noticed all the same. Made from this test's own program, so it
    /// doesn't matter whether that was built with an icon.
    #[cfg(windows)]
    #[test]
    fn an_icon_under_any_id_is_noticed() {
        use windows::probe::{GROUP, put_icon_as, resource_size, strip};
        let dir = std::env::temp_dir().join(format!("reign-exe-icon-any-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let exe = dir.join("game.exe");
        std::fs::copy(std::env::current_exe().unwrap(), &exe).unwrap();
        strip(&exe);
        assert!(!has_exe_icon(&exe), "a stripped program has no icon");
        assert_eq!(resource_size(&exe, GROUP, 1), None);

        let entries = windows_entries(&flat(64, [1, 200, 3, 255]));
        put_icon_as(&exe, &entries, 77, 500);
        assert!(has_exe_icon(&exe), "group 77 counts");
        assert_eq!(resource_size(&exe, GROUP, 77), Some(group_icon(&entries).len() as u32));
        assert_eq!(resource_size(&exe, GROUP, 1), None);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_program_that_isnt_one_is_an_error_not_a_crash() {
        let dir = std::env::temp_dir().join(format!("reign-exe-icon-bad-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let fake = dir.join("fake.exe");
        std::fs::write(&fake, b"not really a game").unwrap();
        let result = set_exe_icon(&fake, icon::default_icon());
        if cfg!(windows) {
            assert!(result.is_err(), "{result:?}");
            assert_eq!(std::fs::read(&fake).unwrap(), b"not really a game", "left alone");
        } else {
            assert!(result.is_ok());
        }
        // A file that isn't there.
        let missing = set_exe_icon(&dir.join("nope.exe"), icon::default_icon());
        assert_eq!(missing.is_err(), cfg!(windows));
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// The real thing: copy a real Windows program (this test's own), put
    /// the project's icon in the copy, and read the resources back: they are
    /// the new icon's, sizes and all.
    #[cfg(windows)]
    #[test]
    fn a_real_program_gets_the_icon() {
        use windows::probe::{GROUP, ICON, resource_size};
        let dir = std::env::temp_dir().join(format!("reign-exe-icon-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let exe = dir.join("game.exe");
        std::fs::copy(std::env::current_exe().unwrap(), &exe).unwrap();
        let original = std::fs::read(&exe).unwrap();

        // A 64 px icon: five sizes (16 to 64), so the group is smaller than
        // the seven-size one the program started with.
        let small = flat(64, [200, 20, 30, 255]);
        set_exe_icon(&exe, &small).expect("the icon is written");
        let entries = windows_entries(&small);
        assert_eq!(entries.len(), 5);
        assert_eq!(resource_size(&exe, GROUP, 1), Some(group_icon(&entries).len() as u32));
        for (i, entry) in entries.iter().enumerate() {
            assert_eq!(
                resource_size(&exe, ICON, i + 1),
                Some(entry.png.len() as u32),
                "size {}",
                entry.size
            );
        }
        assert_ne!(std::fs::read(&exe).unwrap(), original, "the program was changed");

        // The default icon: seven sizes, and its own pixels.
        let image = icon::default_icon();
        set_exe_icon(&exe, image).expect("replaced");
        let entries = windows_entries(image);
        assert_eq!(entries.len(), 7);
        assert_eq!(resource_size(&exe, GROUP, 1), Some(group_icon(&entries).len() as u32));
        for (i, entry) in entries.iter().enumerate() {
            assert_eq!(
                resource_size(&exe, ICON, i + 1),
                Some(entry.png.len() as u32),
                "size {}",
                entry.size
            );
        }
        let _ = std::fs::remove_dir_all(&dir);
    }

    fn version_info() -> VersionInfo {
        VersionInfo {
            numbers: [1, 2, 3, 0],
            version: "1.2.3".into(),
            title: "Space: Rocks!".into(),
            author: "Small Studio".into(),
            exe_name: "space_rocks.exe".into(),
            internal_name: "space_rocks".into(),
        }
    }

    /// A version resource block read back: its key, its value and its
    /// children.
    #[derive(Debug)]
    struct Block {
        key: String,
        is_text: bool,
        value: Vec<u8>,
        children: Vec<Block>,
    }

    fn read_block(bytes: &[u8]) -> Block {
        let word = |at: usize| u16::from_le_bytes([bytes[at], bytes[at + 1]]);
        let length = word(0) as usize;
        assert!(length <= bytes.len(), "a block runs past its parent");
        let is_text = word(4) == 1;
        let mut at = 6;
        let mut key = Vec::new();
        while word(at) != 0 {
            key.push(word(at));
            at += 2;
        }
        at += 2;
        at = at.next_multiple_of(4);
        let value_bytes = if is_text { word(2) as usize * 2 } else { word(2) as usize };
        let value = bytes[at..at + value_bytes].to_vec();
        at += value_bytes;
        let mut children = Vec::new();
        while at.next_multiple_of(4) < length {
            at = at.next_multiple_of(4);
            let child = read_block(&bytes[at..length]);
            at += u16::from_le_bytes([bytes[at], bytes[at + 1]]) as usize;
            children.push(child);
        }
        assert_eq!(at, length, "the children fill the block");
        Block { key: String::from_utf16(&key).unwrap(), is_text, value, children }
    }

    fn text(value: &[u8]) -> String {
        let units: Vec<u16> = value.chunks(2).map(|c| u16::from_le_bytes([c[0], c[1]])).collect();
        String::from_utf16(&units).unwrap().trim_end_matches('\0').to_owned()
    }

    #[test]
    fn the_version_resource_holds_the_numbers_and_the_words() {
        let bytes = version_resource(&version_info());
        let root = read_block(&bytes);
        assert_eq!(root.key, "VS_VERSION_INFO");
        assert_eq!(u16::from_le_bytes([bytes[0], bytes[1]]) as usize, bytes.len());
        // The fixed part: the signature, then 1.2.3.0 as file and product
        // version.
        let fixed: Vec<u32> =
            root.value.chunks(4).map(|c| u32::from_le_bytes(c.try_into().unwrap())).collect();
        assert_eq!(fixed.len(), 13);
        assert_eq!(fixed[0], 0xFEEF_04BD);
        assert_eq!(&fixed[2..6], &[0x0001_0002, 0x0003_0000, 0x0001_0002, 0x0003_0000]);

        let [strings, vars] = &root.children[..] else { panic!("{root:?}") };
        assert_eq!(strings.key, "StringFileInfo");
        let table = &strings.children[0];
        assert_eq!(table.key, "040904B0");
        let found: Vec<(String, String)> =
            table.children.iter().map(|b| (b.key.clone(), text(&b.value))).collect();
        let get = |key: &str| found.iter().find(|(k, _)| k == key).map(|(_, v)| v.as_str());
        assert_eq!(get("ProductName"), Some("Space: Rocks!"));
        assert_eq!(get("FileDescription"), Some("Space: Rocks!"));
        assert_eq!(get("ProductVersion"), Some("1.2.3"));
        assert_eq!(get("FileVersion"), Some("1.2.3"));
        assert_eq!(get("CompanyName"), Some("Small Studio"));
        assert_eq!(get("LegalCopyright"), Some("© Small Studio"));
        assert_eq!(get("OriginalFilename"), Some("space_rocks.exe"));
        assert!(table.children.iter().all(|b| b.is_text));

        assert_eq!(vars.key, "VarFileInfo");
        assert_eq!(vars.children[0].key, "Translation");
        assert_eq!(vars.children[0].value, [0x09, 0x04, 0xB0, 0x04]);

        // No author: no company or copyright.
        let anonymous = VersionInfo { author: " ".into(), ..version_info() };
        let root = read_block(&version_resource(&anonymous));
        let keys: Vec<&str> =
            root.children[0].children[0].children.iter().map(|b| b.key.as_str()).collect();
        assert!(!keys.contains(&"CompanyName") && !keys.contains(&"LegalCopyright"), "{keys:?}");
    }

    /// The real thing: Windows itself reads the details back from a copy of
    /// a real program, as Explorer's Properties → Details does.
    #[cfg(windows)]
    #[test]
    fn a_real_program_gets_the_version() {
        use windows::probe::{VERSION, resource_size, version_string};
        let dir = std::env::temp_dir().join(format!("reign-exe-version-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let exe = dir.join("game.exe");
        std::fs::copy(std::env::current_exe().unwrap(), &exe).unwrap();
        set_exe_icon(&exe, icon::default_icon()).unwrap();

        set_exe_version(&exe, &version_info()).expect("the version is written");
        assert_eq!(version_string(&exe, "ProductName").as_deref(), Some("Space: Rocks!"));
        assert_eq!(version_string(&exe, "ProductVersion").as_deref(), Some("1.2.3"));
        assert_eq!(version_string(&exe, "CompanyName").as_deref(), Some("Small Studio"));
        assert_eq!(version_string(&exe, "LegalCopyright").as_deref(), Some("© Small Studio"));
        assert!(has_exe_icon(&exe), "the icon is kept");

        // Again, with another version: replaced, not added beside.
        let newer = VersionInfo { numbers: [2, 0, 0, 0], version: "2.0".into(), ..version_info() };
        set_exe_version(&exe, &newer).unwrap();
        assert_eq!(version_string(&exe, "ProductVersion").as_deref(), Some("2.0"));
        assert_eq!(resource_size(&exe, VERSION, 1), Some(version_resource(&newer).len() as u32));

        // Not a program: an error, and the file is left alone.
        let fake = dir.join("fake.exe");
        std::fs::write(&fake, b"not really a game").unwrap();
        assert!(set_exe_version(&fake, &newer).is_err());
        assert_eq!(std::fs::read(&fake).unwrap(), b"not really a game");
        let _ = std::fs::remove_dir_all(&dir);
    }
}
