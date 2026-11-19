//! Normal maps and light masks for pixel art: making them from a sprite,
//! editing them, and shading with them.
//!
//! A **normal map** says which way each pixel faces, so a light can shade a
//! flat sprite as if it had shape. It is a normal PNG: red is the x of the
//! normal (right is more red), green is its y with **up** as the bright
//! side (the "OpenGL" way), blue is z (towards the viewer). A flat pixel is
//! (128, 128, 255). Alpha is the sprite's alpha.
//!
//! A **light mask** says how much direct light each pixel receives, from
//! 0 (none) to 1 (all of it), in the red channel (grey images: red, green
//! and blue are equal). It is artistic: it darkens crevices and shading
//! the sprite is painted with, and does not cast shadows.
//!
//! Everything here is plain pixel maths on [`RgbaImage`]s: no GPU, no
//! floats in the simulation, safe to run anywhere (the editor's Lighting
//! tab, tests, a build step).
//!
//! - [`generate_normal_map`] and [`generate_light_mask`] make the maps from
//!   a sprite's colours.
//! - [`Selection`] and the functions in [`modify`] edit them by hand.
//! - [`shade_pixel`] and [`shade_image`] light a sprite for a preview, and
//!   [`compose_material`] packs both maps into the one image the renderer
//!   uses.

mod generate;
mod modify;
mod shade;

pub use generate::{MaskParams, NormalParams, Preset, generate_light_mask, generate_normal_map};
pub use modify::{
    BrightnessMode, Direction, SelectMode, Selection, adjust_brightness, apply_normal,
    average_normals, direction_normal, scale_normal_strength, sphere_facet,
};
pub use shade::{PreviewLight, compose_material, shade_image, shade_pixel};

use crate::RgbaImage;

/// A pixel counts as part of the sprite from this alpha up.
pub const SOLID_ALPHA: u8 = 128;

/// The stored bytes of a flat (facing the viewer) normal.
pub const FLAT: [u8; 3] = [128, 128, 255];

/// How much a pixel's colour weighs as brightness, 0 to 1.
pub fn luminance(r: u8, g: u8, b: u8) -> f32 {
    (0.2126 * f32::from(r) + 0.7152 * f32::from(g) + 0.0722 * f32::from(b)) / 255.0
}

pub(crate) fn is_solid(image: &RgbaImage, i: usize) -> bool {
    image.rgba[i * 4 + 3] >= SOLID_ALPHA
}

/// A unit normal as the three bytes a normal map stores.
pub fn encode_normal(n: [f32; 3]) -> [u8; 3] {
    n.map(|c| ((c * 0.5 + 0.5) * 255.0).round().clamp(0.0, 255.0) as u8)
}

/// The unit normal a normal map's bytes mean (a zero vector is flat).
pub fn decode_normal(rgb: [u8; 3]) -> [f32; 3] {
    let v = rgb.map(|c| f32::from(c) / 255.0 * 2.0 - 1.0);
    let len = (v[0] * v[0] + v[1] * v[1] + v[2] * v[2]).sqrt();
    if len < 1e-6 { [0.0, 0.0, 1.0] } else { [v[0] / len, v[1] / len, v[2] / len] }
}

/// The normal tilted `tilt` degrees from straight at the viewer, towards
/// `heading` degrees: 0 is right, 90 is up (image up), 180 is left.
pub(crate) fn tilted(heading: f32, tilt: f32) -> [f32; 3] {
    let (t, h) = (tilt.to_radians(), heading.to_radians());
    [t.sin() * h.cos(), t.sin() * h.sin(), t.cos()]
}

pub(crate) fn blank(width: u32, height: u32) -> RgbaImage {
    RgbaImage { rgba: vec![0; width as usize * height as usize * 4], width, height }
}

/// For each pixel, whether it is a dark, thin outline pixel: part of a
/// connected run of one dark colour that is never more than two pixels
/// thick. Outlines belong to the surface they enclose, so normals and
/// selections treat them specially.
pub(crate) fn outline_pixels(image: &RgbaImage) -> Vec<bool> {
    generate::outline_pixels(image)
}

#[cfg(test)]
pub(crate) mod fixtures {
    use super::*;

    /// A `w` x `h` image where `f(x, y)` gives each pixel's RGBA.
    pub fn image(w: u32, h: u32, f: impl Fn(u32, u32) -> [u8; 4]) -> RgbaImage {
        let mut rgba = Vec::new();
        for y in 0..h {
            for x in 0..w {
                rgba.extend(f(x, y));
            }
        }
        RgbaImage { rgba, width: w, height: h }
    }

    pub fn pixel(image: &RgbaImage, x: u32, y: u32) -> [u8; 4] {
        let i = (y * image.width + x) as usize * 4;
        [image.rgba[i], image.rgba[i + 1], image.rgba[i + 2], image.rgba[i + 3]]
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn normals_round_trip_through_bytes() {
        assert!(decode_normal(FLAT)[2] > 0.999);
        let n = tilted(90.0, 30.0);
        let back = decode_normal(encode_normal(n));
        for i in 0..3 {
            assert!((n[i] - back[i]).abs() < 0.02, "{n:?} vs {back:?}");
        }
        assert_eq!(encode_normal([0.0, 0.0, 1.0]), FLAT);
    }

    #[test]
    fn heading_zero_is_right_and_ninety_is_up() {
        let right = tilted(0.0, 45.0);
        assert!(right[0] > 0.7 && right[1].abs() < 1e-5);
        let up = tilted(90.0, 45.0);
        assert!(up[1] > 0.7 && up[0].abs() < 1e-5);
    }

    #[test]
    fn luminance_weighs_green_most() {
        assert!(luminance(0, 255, 0) > luminance(255, 0, 0));
        assert!(luminance(255, 0, 0) > luminance(0, 0, 255));
        assert!((luminance(255, 255, 255) - 1.0).abs() < 1e-4);
    }
}
