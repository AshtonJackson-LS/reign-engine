//! Lighting a sprite with its maps: the editor's preview, and the formula
//! the renderer's light shader follows.
//!
//! One point light shines on the sprite. For a pixel:
//!
//! ```text
//! direct  = light.color * light.brightness * falloff(distance) * max(N.L, 0)
//! direct *= lerp(1, mask, mask_influence)
//! result  = albedo * (ambient + direct)
//! ```
//!
//! `L` is the way to the light, with the light `height` pixels in front of
//! the sprite; `falloff` goes from 1 at the light to 0 at `light.falloff`
//! pixels (the same curve the engine's lights use). A pixel with no normal
//! map takes the plain radial light, with no `N.L` term, as lights always
//! did before maps existed.

use super::{SOLID_ALPHA, decode_normal, is_solid};
use crate::RgbaImage;

/// The preview's one point light, in image pixels (y grows downwards).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct PreviewLight {
    pub x: f32,
    pub y: f32,
    /// How far in front of the sprite it is, in pixels.
    pub height: f32,
    /// 0 to 1 per channel.
    pub color: [f32; 3],
    pub brightness: f32,
    /// The distance, in pixels, where its light runs out.
    pub falloff: f32,
}

impl Default for PreviewLight {
    fn default() -> Self {
        PreviewLight {
            x: 0.0,
            y: 0.0,
            height: 25.0,
            color: [1.0; 3],
            brightness: 2.5,
            falloff: 70.0,
        }
    }
}

/// How much light reaches `d2` (the squared distance over the squared
/// falloff): 1 at the light, 0 at the edge.
pub fn attenuation(d2: f32) -> f32 {
    if d2 >= 1.0 {
        return 0.0;
    }
    let inv = 1.0 - d2;
    inv * inv / (1.0 + 8.0 * d2)
}

/// The colour of one pixel under `light`. `albedo` is 0 to 1 per channel;
/// `normal` is `None` for a pixel without a normal map; `mask` is 0 to 1;
/// `at` is where the pixel is in image pixels.
pub fn shade_pixel(
    albedo: [f32; 3],
    normal: Option<[f32; 3]>,
    mask: f32,
    at: (f32, f32),
    light: &PreviewLight,
    ambient: f32,
    mask_influence: f32,
) -> [f32; 3] {
    let (dx, dy) = (light.x - at.0, light.y - at.1);
    let falloff = light.falloff.max(1e-3);
    let fall = attenuation((dx * dx + dy * dy) / (falloff * falloff));
    let facing = match normal {
        Some(n) => {
            // Image y grows downwards; normals have up as positive.
            let (lx, ly, lz) = (dx, -dy, light.height);
            let len = (lx * lx + ly * ly + lz * lz).sqrt().max(1e-6);
            ((n[0] * lx + n[1] * ly + n[2] * lz) / len).max(0.0)
        }
        None => 1.0,
    };
    let reception = 1.0 + (mask.clamp(0.0, 1.0) - 1.0) * mask_influence.clamp(0.0, 1.0);
    let direct = light.brightness * fall * facing * reception;
    [0, 1, 2].map(|c| (albedo[c] * (ambient + light.color[c] * direct)).clamp(0.0, 1.0))
}

fn channels(image: &RgbaImage, i: usize) -> [f32; 3] {
    [0, 1, 2].map(|c| f32::from(image.rgba[i * 4 + c]) / 255.0)
}

/// Lights the whole sprite: `albedo` with its optional maps (the same size,
/// or ignored). See-through pixels stay as they were. A sprite with either
/// map treats pixels without a normal as flat.
pub fn shade_image(
    albedo: &RgbaImage,
    normal: Option<&RgbaImage>,
    mask: Option<&RgbaImage>,
    light: &PreviewLight,
    ambient: f32,
    mask_influence: f32,
) -> RgbaImage {
    let same = |m: &&RgbaImage| (m.width, m.height) == (albedo.width, albedo.height);
    let (normal, mask) = (normal.filter(same), mask.filter(same));
    let mapped = normal.is_some() || mask.is_some();
    let w = albedo.width as usize;
    let mut out = albedo.clone();
    for i in 0..w * albedo.height as usize {
        if !is_solid(albedo, i) {
            continue;
        }
        let n = mapped.then(|| {
            normal.map_or([0.0, 0.0, 1.0], |m| {
                decode_normal([m.rgba[i * 4], m.rgba[i * 4 + 1], m.rgba[i * 4 + 2]])
            })
        });
        let reception = mask.map_or(1.0, |m| f32::from(m.rgba[i * 4]) / 255.0);
        let at = ((i % w) as f32 + 0.5, (i / w) as f32 + 0.5);
        let lit =
            shade_pixel(channels(albedo, i), n, reception, at, light, ambient, mask_influence);
        for c in 0..3 {
            out.rgba[i * 4 + c] = (lit[c] * 255.0).round() as u8;
        }
    }
    out
}

/// Packs a sprite's maps into the one image the renderer samples: red and
/// green are the normal's x and y, blue is the mask, and alpha is 255 for
/// pixels that have maps (the sprite's solid pixels) and 0 for the rest,
/// which light the old way. A missing normal map is flat, a missing mask
/// lets all the light in. Maps of another size than `albedo` are ignored.
pub fn compose_material(
    albedo: &RgbaImage,
    normal: Option<&RgbaImage>,
    mask: Option<&RgbaImage>,
) -> RgbaImage {
    let same = |m: &&RgbaImage| (m.width, m.height) == (albedo.width, albedo.height);
    let (normal, mask) = (normal.filter(same), mask.filter(same));
    let mut out = super::blank(albedo.width, albedo.height);
    for i in 0..albedo.width as usize * albedo.height as usize {
        if albedo.rgba[i * 4 + 3] < SOLID_ALPHA {
            continue;
        }
        let (r, g) = normal.map_or((128, 128), |m| (m.rgba[i * 4], m.rgba[i * 4 + 1]));
        let b = mask.map_or(255, |m| m.rgba[i * 4]);
        out.rgba[i * 4..i * 4 + 4].copy_from_slice(&[r, g, b, 255]);
    }
    out
}

#[cfg(test)]
mod tests {
    use super::super::encode_normal;
    use super::super::fixtures::{image, pixel};
    use super::*;

    fn light_at(x: f32, y: f32) -> PreviewLight {
        PreviewLight { x, y, height: 20.0, color: [1.0; 3], brightness: 1.0, falloff: 100.0 }
    }

    #[test]
    fn falloff_is_one_at_the_light_and_zero_at_its_edge() {
        assert!((attenuation(0.0) - 1.0).abs() < 1e-6);
        assert_eq!(attenuation(1.0), 0.0);
        assert_eq!(attenuation(4.0), 0.0);
        assert!(attenuation(0.25) > attenuation(0.5));
    }

    #[test]
    fn a_surface_facing_the_light_is_brighter_than_one_facing_away() {
        // The light is to the right of the pixel.
        let light = light_at(30.0, 10.0);
        let toward = Some([0.7, 0.0, 0.7]);
        let away = Some([-0.7, 0.0, 0.7]);
        let at = (10.0, 10.0);
        let lit = |n| shade_pixel([1.0; 3], n, 1.0, at, &light, 0.0, 1.0)[0];
        assert!(lit(toward) > lit(Some([0.0, 0.0, 1.0])));
        assert!(lit(Some([0.0, 0.0, 1.0])) > lit(away));
        assert!(lit(Some([-1.0, 0.0, 0.0])) == 0.0, "side on or behind gets nothing");
    }

    #[test]
    fn image_y_grows_down_but_normals_point_up() {
        // The light is above the pixel (smaller y); an up-facing normal is lit.
        let light = light_at(10.0, 0.0);
        let at = (10.0, 10.0);
        let up = shade_pixel([1.0; 3], Some([0.0, 0.7, 0.7]), 1.0, at, &light, 0.0, 1.0)[0];
        let down = shade_pixel([1.0; 3], Some([0.0, -0.7, 0.7]), 1.0, at, &light, 0.0, 1.0)[0];
        assert!(up > down);
    }

    #[test]
    fn the_mask_only_blocks_direct_light_and_influence_scales_it() {
        let light = light_at(10.0, 10.0);
        let at = (10.0, 10.0);
        let shade = |mask, influence| {
            shade_pixel([0.5; 3], Some([0.0, 0.0, 1.0]), mask, at, &light, 0.2, influence)[0]
        };
        assert!((shade(0.0, 1.0) - 0.5 * 0.2).abs() < 1e-5, "ambient only");
        assert!(shade(0.0, 0.0) > shade(0.0, 1.0), "influence 0 ignores the mask");
        assert!(shade(0.5, 1.0) > shade(0.0, 1.0) && shade(0.5, 1.0) < shade(1.0, 1.0));
    }

    #[test]
    fn beyond_the_falloff_only_ambient_remains() {
        let light = light_at(0.0, 0.0);
        let far = shade_pixel([0.8; 3], None, 1.0, (500.0, 0.0), &light, 0.15, 1.0);
        assert!((far[0] - 0.8 * 0.15).abs() < 1e-5);
    }

    #[test]
    fn a_pixel_without_a_normal_map_takes_the_plain_radial_light() {
        let light = light_at(10.0, 10.0);
        let plain = shade_pixel([0.5; 3], None, 1.0, (10.0, 10.0), &light, 0.0, 1.0)[0];
        assert!((plain - 0.5).abs() < 1e-5, "full strength under the light");
    }

    #[test]
    fn shading_an_image_keeps_alpha_and_clear_pixels() {
        let albedo = image(2, 1, |x, _| if x == 0 { [200, 100, 50, 255] } else { [9, 9, 9, 0] });
        let normal = image(2, 1, |_, _| [128, 128, 255, 255]);
        let lit = shade_image(&albedo, Some(&normal), None, &light_at(0.5, 0.5), 0.1, 1.0);
        assert_eq!(pixel(&lit, 1, 0), [9, 9, 9, 0]);
        assert_eq!(pixel(&lit, 0, 0)[3], 255);
        assert!(pixel(&lit, 0, 0)[0] > 0);
        // A map of the wrong size is ignored, not misread.
        let wrong = image(3, 3, |_, _| [255, 255, 255, 255]);
        let plain = shade_image(&albedo, Some(&wrong), Some(&wrong), &light_at(0.5, 0.5), 0.1, 1.0);
        let none = shade_image(&albedo, None, None, &light_at(0.5, 0.5), 0.1, 1.0);
        assert_eq!(plain, none);
    }

    #[test]
    fn composing_a_material_packs_normal_xy_mask_and_a_has_maps_flag() {
        let albedo = image(2, 1, |x, _| if x == 0 { [1, 2, 3, 255] } else { [1, 2, 3, 40] });
        let normal = image(2, 1, |_, _| {
            let n = encode_normal([0.6, 0.0, 0.8]);
            [n[0], n[1], n[2], 255]
        });
        let mask = image(2, 1, |_, _| [77, 77, 77, 255]);
        let m = compose_material(&albedo, Some(&normal), Some(&mask));
        let expected = encode_normal([0.6, 0.0, 0.8]);
        assert_eq!(pixel(&m, 0, 0), [expected[0], expected[1], 77, 255]);
        assert_eq!(pixel(&m, 1, 0), [0, 0, 0, 0], "see-through pixels have no maps");
        // Only a mask: flat normals. Only a normal: all the light.
        assert_eq!(pixel(&compose_material(&albedo, None, Some(&mask)), 0, 0), [128, 128, 77, 255]);
        assert_eq!(pixel(&compose_material(&albedo, Some(&normal), None), 0, 0)[2], 255);
    }
}
