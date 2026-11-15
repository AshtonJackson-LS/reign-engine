//! Project icons: the Reign default, a project's own `icon.png`, and what
//! the engine does with either.
//!
//! A project's icon is the file `icon.png` in its folder, next to
//! `ReignProject.ron`; with none, the Reign project icon is used. It becomes
//! the editor's and the game's window icon, and Export Game puts it in the
//! exported game (see `export.rs`).
//!
//! The rules for a project's own icon are deliberately strict, so it looks
//! right at every size it will be shown at:
//! - a **PNG** (the one format every platform's icon tools can start from,
//!   with transparency; JPEG and the rest are not accepted),
//! - **square**,
//! - from [`MIN_SIZE`] to [`MAX_SIZE`] pixels on a side (256 or 512 is a good
//!   choice),
//! - a file of at most [`MAX_BYTES`].
//!
//! [`check`] applies them and says in plain words which one an image breaks.

use std::path::Path;
use std::sync::OnceLock;

use reign::assets::RgbaImage;

/// The file name of a project's own icon, in the project folder.
pub const FILE: &str = "icon.png";

/// The smallest side of an icon, in pixels.
pub const MIN_SIZE: u32 = 64;
/// The largest side of an icon, in pixels.
pub const MAX_SIZE: u32 = 1024;
/// The largest icon file.
pub const MAX_BYTES: usize = 8 * 1024 * 1024;

/// The Reign project icon: used when a project has none of its own.
const DEFAULT_PNG: &[u8] = include_bytes!("../../assets/icon/project-512.png");

/// Why an image can't be a project's icon.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum IconError {
    /// The file is bigger than [`MAX_BYTES`].
    TooHeavy {
        bytes: usize,
    },
    /// Not a PNG (a JPEG, a GIF, an .ico, or something else).
    NotPng,
    /// A PNG the decoder couldn't read, with its reason.
    Unreadable(String),
    NotSquare {
        width: u32,
        height: u32,
    },
    TooSmall {
        side: u32,
    },
    TooBig {
        side: u32,
    },
    /// Every pixel is fully transparent.
    Invisible,
}

impl std::fmt::Display for IconError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            IconError::TooHeavy { bytes } => write!(
                f,
                "That file is {:.1} MB; an icon can be at most {} MB.",
                *bytes as f64 / (1024.0 * 1024.0),
                MAX_BYTES / (1024 * 1024)
            ),
            IconError::NotPng => f.write_str("The icon must be a PNG image (a .png file)."),
            IconError::Unreadable(why) => write!(f, "That PNG can't be read: {why}"),
            IconError::NotSquare { width, height } => {
                write!(f, "The icon must be square; that image is {width} × {height} pixels.")
            }
            IconError::TooSmall { side } => write!(
                f,
                "That image is {side} × {side} pixels; an icon must be at least \
                 {MIN_SIZE} × {MIN_SIZE}."
            ),
            IconError::TooBig { side } => write!(
                f,
                "That image is {side} × {side} pixels; an icon can be at most \
                 {MAX_SIZE} × {MAX_SIZE}."
            ),
            IconError::Invisible => f.write_str("That image is completely transparent."),
        }
    }
}

impl std::error::Error for IconError {}

/// Whether `bytes` can be a project's icon, and the image if so. The size is
/// checked from the PNG's header before the pixels are decoded, so a huge
/// image is turned away without being unpacked. Greyscale, RGB, palette and
/// 16-bit PNGs are all accepted and come back as 8-bit RGBA.
pub fn check(bytes: &[u8]) -> Result<RgbaImage, IconError> {
    decode(bytes, true)
}

/// The PNG's pixels without the size limits, for the small sizes the
/// engine makes itself (tests read them back).
#[cfg(test)]
pub(crate) fn check_any_size(bytes: &[u8]) -> Result<RgbaImage, IconError> {
    decode(bytes, false)
}

fn decode(bytes: &[u8], limits: bool) -> Result<RgbaImage, IconError> {
    if bytes.len() > MAX_BYTES {
        return Err(IconError::TooHeavy { bytes: bytes.len() });
    }
    if !bytes.starts_with(&[0x89, b'P', b'N', b'G', 0x0D, 0x0A, 0x1A, 0x0A]) {
        return Err(IconError::NotPng);
    }
    let unreadable = |e: png::DecodingError| IconError::Unreadable(e.to_string());
    let mut decoder = png::Decoder::new(std::io::Cursor::new(bytes));
    decoder.set_transformations(png::Transformations::EXPAND | png::Transformations::STRIP_16);
    let mut reader = decoder.read_info().map_err(unreadable)?;
    let (width, height) = {
        let info = reader.info();
        (info.width, info.height)
    };
    if width != height {
        return Err(IconError::NotSquare { width, height });
    }
    if limits && width < MIN_SIZE {
        return Err(IconError::TooSmall { side: width });
    }
    if limits && width > MAX_SIZE {
        return Err(IconError::TooBig { side: width });
    }
    let mut buffer = vec![
        0;
        reader.output_buffer_size().ok_or_else(|| {
            IconError::Unreadable("it is too large to unpack".into())
        })?
    ];
    let frame = reader.next_frame(&mut buffer).map_err(unreadable)?;
    buffer.truncate(frame.buffer_size());
    let rgba = to_rgba(&buffer, frame.color_type, (width * height) as usize)?;
    if rgba.as_chunks::<4>().0.iter().all(|p| p[3] == 0) {
        return Err(IconError::Invisible);
    }
    Ok(RgbaImage { rgba, width, height })
}

/// 8-bit pixels of any colour type as RGBA.
fn to_rgba(pixels: &[u8], color: png::ColorType, count: usize) -> Result<Vec<u8>, IconError> {
    let mut out = Vec::with_capacity(count * 4);
    match color {
        png::ColorType::Rgba => out.extend_from_slice(pixels),
        png::ColorType::Rgb => {
            for p in pixels.as_chunks::<3>().0 {
                out.extend_from_slice(&[p[0], p[1], p[2], 255]);
            }
        }
        png::ColorType::Grayscale => {
            for &g in pixels {
                out.extend_from_slice(&[g, g, g, 255]);
            }
        }
        png::ColorType::GrayscaleAlpha => {
            for p in pixels.as_chunks::<2>().0 {
                out.extend_from_slice(&[p[0], p[0], p[0], p[1]]);
            }
        }
        // EXPAND turns palettes into RGB(A) before they get here.
        png::ColorType::Indexed => {
            return Err(IconError::Unreadable("it uses a palette that can't be expanded".into()));
        }
    }
    if out.len() != count * 4 {
        return Err(IconError::Unreadable("its pixel data is the wrong size".into()));
    }
    Ok(out)
}

/// The Reign project icon.
pub fn default_icon() -> &'static RgbaImage {
    static ICON: OnceLock<RgbaImage> = OnceLock::new();
    ICON.get_or_init(|| check(DEFAULT_PNG).expect("the built-in project icon is a valid icon"))
}

/// The project in `root`'s own icon: its [`FILE`], if there is a good one. A
/// bad file is logged and skipped, never fatal.
pub fn own_icon(root: &Path) -> Option<RgbaImage> {
    let path = root.join(FILE);
    match std::fs::read(&path) {
        Ok(bytes) => match check(&bytes) {
            Ok(image) => return Some(image),
            Err(e) => log::warn!("not using {}: {e}", path.display()),
        },
        // No icon file is the normal case.
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
        Err(e) => log::warn!("can't read {}: {e}", path.display()),
    }
    None
}

/// The icon for the project in `root`: its own if it has one, else the
/// default.
pub fn project_icon(root: &Path) -> RgbaImage {
    own_icon(root).unwrap_or_else(|| default_icon().clone())
}

/// `image` scaled to `size` × `size`, averaging the source pixels each new
/// pixel covers (with fractional edges) in premultiplied alpha, so
/// transparent edges don't darken or fringe. Used both to shrink (the usual
/// case) and, for a small source, to enlarge.
pub fn resize(image: &RgbaImage, size: u32) -> RgbaImage {
    let size = size.max(1);
    if image.width == size && image.height == size {
        return image.clone();
    }
    let (sw, sh) = (image.width as usize, image.height as usize);
    let (x_scale, y_scale) = (sw as f64 / size as f64, sh as f64 / size as f64);
    let mut out = Vec::with_capacity((size * size * 4) as usize);
    for oy in 0..size as usize {
        let (y0, y1) = (oy as f64 * y_scale, (oy + 1) as f64 * y_scale);
        for ox in 0..size as usize {
            let (x0, x1) = (ox as f64 * x_scale, (ox + 1) as f64 * x_scale);
            let (mut r, mut g, mut b, mut a, mut weight) = (0.0, 0.0, 0.0, 0.0, 0.0);
            for sy in y0.floor() as usize..(y1.ceil() as usize).min(sh) {
                let wy = (y1.min(sy as f64 + 1.0) - y0.max(sy as f64)).max(0.0);
                for sx in x0.floor() as usize..(x1.ceil() as usize).min(sw) {
                    let wx = (x1.min(sx as f64 + 1.0) - x0.max(sx as f64)).max(0.0);
                    let w = wx * wy;
                    let p = &image.rgba[(sy * sw + sx) * 4..][..4];
                    let alpha = p[3] as f64 / 255.0;
                    r += p[0] as f64 * alpha * w;
                    g += p[1] as f64 * alpha * w;
                    b += p[2] as f64 * alpha * w;
                    a += alpha * w;
                    weight += w;
                }
            }
            if weight == 0.0 || a == 0.0 {
                out.extend_from_slice(&[0, 0, 0, 0]);
            } else {
                // Back from premultiplied.
                out.extend_from_slice(&[
                    (r / a).round().clamp(0.0, 255.0) as u8,
                    (g / a).round().clamp(0.0, 255.0) as u8,
                    (b / a).round().clamp(0.0, 255.0) as u8,
                    (a / weight * 255.0).round().clamp(0.0, 255.0) as u8,
                ]);
            }
        }
    }
    RgbaImage { rgba: out, width: size, height: size }
}

/// `image` as PNG bytes.
pub fn encode_png(image: &RgbaImage) -> Vec<u8> {
    let mut out = Vec::new();
    let mut encoder = png::Encoder::new(&mut out, image.width, image.height);
    encoder.set_color(png::ColorType::Rgba);
    encoder.set_depth(png::BitDepth::Eight);
    encoder
        .write_header()
        .and_then(|mut writer| writer.write_image_data(&image.rgba))
        .expect("encoding to memory can't fail");
    out
}

/// The window icon for `image`: 64 × 64, which the system scales for its
/// title bar (16-32 px) and taskbar (32-48 px). A little over those sizes
/// keeps it sharp on high-density screens.
pub fn window_icon(image: &RgbaImage) -> RgbaImage {
    resize(image, 64)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A PNG of the given colour type and size, filled by `pixel(x, y)`
    /// (one value per sample), built here so the tests need no files.
    fn make_png(
        width: u32,
        height: u32,
        color: png::ColorType,
        depth: png::BitDepth,
        palette: Option<Vec<u8>>,
        data: Vec<u8>,
    ) -> Vec<u8> {
        let mut out = Vec::new();
        let mut encoder = png::Encoder::new(&mut out, width, height);
        encoder.set_color(color);
        encoder.set_depth(depth);
        if let Some(palette) = palette {
            encoder.set_palette(palette);
        }
        let mut writer = encoder.write_header().unwrap();
        writer.write_image_data(&data).unwrap();
        writer.finish().unwrap();
        out
    }

    fn rgba_png(side: u32, pixel: [u8; 4]) -> Vec<u8> {
        let data = pixel.iter().copied().cycle().take((side * side * 4) as usize).collect();
        make_png(side, side, png::ColorType::Rgba, png::BitDepth::Eight, None, data)
    }

    #[test]
    fn the_built_in_icon_is_a_good_icon() {
        let icon = default_icon();
        assert_eq!((icon.width, icon.height), (512, 512));
        // Rounded corners: transparent at the corner, opaque at the middle.
        assert_eq!(icon.rgba[3], 0, "the corner is transparent");
        let middle = (256 * 512 + 256) * 4;
        assert_eq!(icon.rgba[middle + 3], 255);
        // The same bytes pass the rules a project's own icon has to pass.
        assert!(check(DEFAULT_PNG).is_ok());
    }

    #[test]
    fn a_good_icon_is_accepted_at_every_allowed_size() {
        for side in [MIN_SIZE, 100, 256, 512, MAX_SIZE] {
            let image = check(&rgba_png(side, [200, 30, 40, 255])).unwrap();
            assert_eq!((image.width, image.height), (side, side));
            assert_eq!(&image.rgba[..4], &[200, 30, 40, 255]);
        }
    }

    #[test]
    fn each_rule_has_its_own_plain_message() {
        let cases = [
            (rgba_png(63, [1, 2, 3, 255]), IconError::TooSmall { side: 63 }),
            (rgba_png(1025, [1, 2, 3, 255]), IconError::TooBig { side: 1025 }),
            (
                make_png(
                    128,
                    64,
                    png::ColorType::Rgba,
                    png::BitDepth::Eight,
                    None,
                    vec![9; 128 * 64 * 4],
                ),
                IconError::NotSquare { width: 128, height: 64 },
            ),
            (rgba_png(64, [10, 20, 30, 0]), IconError::Invisible),
            (b"\xff\xd8\xff\xe0 not a png".to_vec(), IconError::NotPng),
            (b"GIF89a".to_vec(), IconError::NotPng),
            (Vec::new(), IconError::NotPng),
        ];
        for (bytes, expected) in cases {
            let error = check(&bytes).unwrap_err();
            assert_eq!(error, expected);
            assert!(!error.to_string().is_empty());
        }
        assert_eq!(
            IconError::NotSquare { width: 512, height: 256 }.to_string(),
            "The icon must be square; that image is 512 × 256 pixels."
        );
        assert!(IconError::TooSmall { side: 32 }.to_string().contains("at least 64 × 64"));
        assert!(IconError::TooBig { side: 2048 }.to_string().contains("at most 1024 × 1024"));
    }

    #[test]
    fn a_huge_file_is_turned_away_before_anything_is_decoded() {
        let mut bytes = rgba_png(64, [1, 2, 3, 255]);
        bytes.resize(MAX_BYTES + 1, 0);
        assert_eq!(check(&bytes), Err(IconError::TooHeavy { bytes: MAX_BYTES + 1 }));
        // The message gives the size in MB.
        assert!(check(&bytes).unwrap_err().to_string().contains("8.0 MB"));
    }

    #[test]
    fn a_truncated_or_damaged_png_is_unreadable_not_a_crash() {
        let good = rgba_png(64, [5, 6, 7, 255]);
        for cut in [8, 20, good.len() / 2, good.len() - 5] {
            match check(&good[..cut]) {
                Err(IconError::Unreadable(_)) => {}
                other => panic!("cut at {cut}: {other:?}"),
            }
        }
        // Valid signature, nonsense after it.
        let mut junk = vec![0x89, b'P', b'N', b'G', 0x0D, 0x0A, 0x1A, 0x0A];
        junk.extend_from_slice(&[0xAB; 64]);
        assert!(matches!(check(&junk), Err(IconError::Unreadable(_))));
    }

    #[test]
    fn other_colour_types_and_depths_come_back_as_rgba() {
        let side = 64u32;
        let n = (side * side) as usize;
        // RGB (no alpha): opaque.
        let rgb = make_png(
            side,
            side,
            png::ColorType::Rgb,
            png::BitDepth::Eight,
            None,
            [10, 20, 30].repeat(n),
        );
        assert_eq!(&check(&rgb).unwrap().rgba[..4], &[10, 20, 30, 255]);
        // Greyscale and greyscale + alpha.
        let grey = make_png(
            side,
            side,
            png::ColorType::Grayscale,
            png::BitDepth::Eight,
            None,
            vec![77; n],
        );
        assert_eq!(&check(&grey).unwrap().rgba[..4], &[77, 77, 77, 255]);
        let grey_a = make_png(
            side,
            side,
            png::ColorType::GrayscaleAlpha,
            png::BitDepth::Eight,
            None,
            [90, 200].repeat(n),
        );
        assert_eq!(&check(&grey_a).unwrap().rgba[..4], &[90, 90, 90, 200]);
        // A palette image.
        let indexed = make_png(
            side,
            side,
            png::ColorType::Indexed,
            png::BitDepth::Eight,
            Some(vec![0, 0, 0, 200, 100, 50]),
            vec![1; n],
        );
        assert_eq!(&check(&indexed).unwrap().rgba[..4], &[200, 100, 50, 255]);
        // 16 bits a channel: reduced to 8.
        let deep = make_png(
            side,
            side,
            png::ColorType::Rgba,
            png::BitDepth::Sixteen,
            None,
            [0x80, 0x00, 0x40, 0x00, 0xC0, 0x00, 0xFF, 0xFF].repeat(n),
        );
        assert_eq!(&check(&deep).unwrap().rgba[..4], &[0x80, 0x40, 0xC0, 255]);
    }

    #[test]
    fn a_project_without_an_icon_file_gets_the_default() {
        let dir = std::env::temp_dir().join(format!("reign-icon-none-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        assert_eq!(project_icon(&dir), *default_icon());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_project_uses_its_own_icon_and_skips_a_bad_one() {
        let dir = std::env::temp_dir().join(format!("reign-icon-own-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join(FILE), rgba_png(128, [9, 99, 199, 255])).unwrap();
        let own = project_icon(&dir);
        assert_eq!((own.width, &own.rgba[..4]), (128, &[9, 99, 199, 255][..]));
        // Not square: back to the default, no panic.
        std::fs::write(
            dir.join(FILE),
            make_png(
                100,
                64,
                png::ColorType::Rgba,
                png::BitDepth::Eight,
                None,
                vec![1; 100 * 64 * 4],
            ),
        )
        .unwrap();
        assert_eq!(project_icon(&dir), *default_icon());
        // Not a PNG at all.
        std::fs::write(dir.join(FILE), b"GIF89a....").unwrap();
        assert_eq!(project_icon(&dir), *default_icon());
        let _ = std::fs::remove_dir_all(&dir);
    }

    fn image(width: u32, height: u32, f: impl Fn(u32, u32) -> [u8; 4]) -> RgbaImage {
        let mut rgba = Vec::new();
        for y in 0..height {
            for x in 0..width {
                rgba.extend_from_slice(&f(x, y));
            }
        }
        RgbaImage { rgba, width, height }
    }

    #[test]
    fn resizing_keeps_colours_and_averages_detail() {
        // Flat colour stays that colour at any size.
        let flat = image(100, 100, |_, _| [40, 80, 120, 255]);
        for size in [16, 33, 64, 99, 100, 256] {
            let small = resize(&flat, size);
            assert_eq!((small.width, small.height), (size, size));
            assert!(small.rgba.chunks(4).all(|p| p == [40, 80, 120, 255]), "size {size}");
        }
        // A black/white checkerboard of 1-pixel squares averages to grey.
        let checker =
            image(
                64,
                64,
                |x, y| if (x + y) % 2 == 0 { [0, 0, 0, 255] } else { [255, 255, 255, 255] },
            );
        let grey = resize(&checker, 16);
        assert!(grey.rgba.chunks(4).all(|p| (p[0] as i32 - 128).abs() <= 1 && p[3] == 255));
        // Half red, half blue, cut cleanly down the middle.
        let split = image(64, 64, |x, _| if x < 32 { [255, 0, 0, 255] } else { [0, 0, 255, 255] });
        let s = resize(&split, 8);
        assert_eq!(&s.rgba[0..4], &[255, 0, 0, 255]);
        assert_eq!(&s.rgba[7 * 4..8 * 4], &[0, 0, 255, 255]);
    }

    #[test]
    fn transparent_edges_do_not_darken_or_fringe() {
        // An opaque red square on a fully transparent background (whose
        // hidden colour is black): shrunk, the edge pixels are still red,
        // only less opaque.
        let img = image(64, 64, |x, y| {
            if (16..48).contains(&x) && (16..48).contains(&y) {
                [255, 0, 0, 255]
            } else {
                [0, 0, 0, 0]
            }
        });
        let small = resize(&img, 16);
        for p in small.rgba.chunks(4) {
            if p[3] > 0 {
                assert_eq!(&p[..3], &[255, 0, 0], "{p:?}");
            }
        }
        // The corner stays empty, the middle stays solid.
        assert_eq!(small.rgba[3], 0);
        assert_eq!(small.rgba[(8 * 16 + 8) * 4 + 3], 255);
    }

    #[test]
    fn a_half_transparent_pixel_keeps_its_alpha() {
        let img = image(64, 64, |_, _| [100, 150, 200, 128]);
        let small = resize(&img, 32);
        assert!(small.rgba.chunks(4).all(|p| p == [100, 150, 200, 128]));
    }

    #[test]
    fn enlarging_a_small_image_works_too() {
        let img = image(64, 64, |x, _| if x < 32 { [255, 0, 0, 255] } else { [0, 255, 0, 255] });
        let big = resize(&img, 256);
        assert_eq!((big.width, big.height), (256, 256));
        assert_eq!(&big.rgba[0..4], &[255, 0, 0, 255]);
        assert_eq!(&big.rgba[255 * 4..256 * 4], &[0, 255, 0, 255]);
    }

    #[test]
    fn encoding_and_reading_back_gives_the_same_pixels() {
        let img = image(64, 64, |x, y| {
            [x as u8 * 3, y as u8 * 3, (x + y) as u8, if x > 5 { 255 } else { 90 }]
        });
        let back = check(&encode_png(&img)).unwrap();
        assert_eq!(back, img);
    }

    #[test]
    fn the_window_icon_is_64_square() {
        let w = window_icon(default_icon());
        assert_eq!((w.width, w.height), (64, 64));
        assert_eq!(w.rgba.len(), 64 * 64 * 4);
    }
}
