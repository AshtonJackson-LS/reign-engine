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
#[cfg(any(windows, test))]
pub fn group_icon(entries: &[IconEntry]) -> Vec<u8> {
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
        out.extend_from_slice(&(i as u16 + 1).to_le_bytes()); // resource id
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
/// embed its own when it is built). Always false elsewhere, where there is
/// none to have.
pub fn has_exe_icon(exe: &std::path::Path) -> bool {
    #[cfg(windows)]
    {
        windows::resource_size(exe, windows::GROUP, 1).is_some()
            || windows::resource_size(exe, windows::GROUP, 101).is_some()
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

#[cfg(windows)]
mod windows {
    //! The three resource-update calls of the Windows API, which are the way
    //! to change a program's resources (there is no safe equivalent in the
    //! standard library). They are only given pointers to data this module
    //! owns for the length of each call.

    use std::ffi::c_void;
    use std::os::windows::ffi::OsStrExt;
    use std::path::Path;

    use super::{IconEntry, group_icon};

    // Resource types, as the Windows API names them (`MAKEINTRESOURCE`).
    const RT_ICON: usize = 3;
    const RT_GROUP_ICON: usize = 14;

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

    pub(super) fn update(exe: &Path, entries: &[IconEntry]) -> Result<(), String> {
        if entries.is_empty() {
            return Err("the icon has no sizes to write".into());
        }
        let wide: Vec<u16> = exe.as_os_str().encode_wide().chain(Some(0)).collect();
        // SAFETY: `wide` is a NUL-terminated path that outlives the call.
        // `false` keeps the program's other resources.
        let handle = unsafe { BeginUpdateResourceW(wide.as_ptr(), 0) };
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
        for (i, entry) in entries.iter().enumerate() {
            put(RT_ICON, i + 1, &entry.png);
        }
        put(RT_GROUP_ICON, 1, &group_icon(entries));
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
    unsafe extern "system" {
        fn LoadLibraryExW(file_name: *const u16, file: *mut c_void, flags: u32) -> *mut c_void;
        fn FreeLibrary(module: *mut c_void) -> i32;
        fn FindResourceW(module: *mut c_void, name: *const u16, kind: *const u16) -> *mut c_void;
        fn SizeofResource(module: *mut c_void, resource: *mut c_void) -> u32;
    }

    const LOAD_LIBRARY_AS_DATAFILE: u32 = 0x2;
    #[cfg(test)]
    pub(super) const ICON: usize = RT_ICON;
    pub(super) const GROUP: usize = RT_GROUP_ICON;

    /// The size in bytes of resource `id` of `kind` in `exe`, if it has one.
    pub(super) fn resource_size(exe: &Path, kind: usize, id: usize) -> Option<u32> {
        let wide: Vec<u16> = exe.as_os_str().encode_wide().chain(Some(0)).collect();
        // SAFETY: as above; the module is loaded as data only (nothing runs),
        // freed before returning, and the resource isn't used after that.
        unsafe {
            let module =
                LoadLibraryExW(wide.as_ptr(), std::ptr::null_mut(), LOAD_LIBRARY_AS_DATAFILE);
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
        // This test's own program is built with the engine's icon (see build.rs).
        assert_eq!(has_exe_icon(&std::env::current_exe().unwrap()), cfg!(windows));
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

    /// The real thing: copy a real Windows program (this test's own, which
    /// already has the engine's icon), put the project's icon in the copy,
    /// and read the resources back: they are the new icon's, sizes and all.
    #[cfg(windows)]
    #[test]
    fn a_real_program_gets_the_icon() {
        use windows::{GROUP, ICON, resource_size};
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
}
