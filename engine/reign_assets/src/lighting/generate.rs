//! Making a normal map and a light mask from a sprite's colours.
//!
//! The normal map is an *estimate*: pixel art carries no real depth, so the
//! shape is guessed from brightness and from the sprite's silhouette, then
//! simplified into flat facets (pixel art is read as a few surfaces, not a
//! smooth gradient). Large complex or hard-surface art may need touching up
//! by hand with the [`super::modify`] tools.
//!
//! How it works, in order:
//! 1. Pixels of one colour that touch form a *cluster*.
//! 2. A height guess per pixel: brightness (a little smoothed) mixed with a
//!    dome that rises from the silhouette inwards.
//! 3. The height's slope gives each pixel a direction, quantised to one of
//!    eight headings and a few tilt steps. Each cluster takes its average
//!    slope, so a flat-coloured area is one surface. With *Facet Splitting*
//!    big clusters keep their own per-pixel directions and split into
//!    facets where the direction changes.
//! 4. Tiny facets merge into the neighbour they share the most border with.
//! 5. With *Merge Outlines*, thin dark outlines take the normal of the
//!    surface beside them instead of their own.
//! 6. *Edge Width* bevels the silhouette outwards.

use std::collections::BTreeMap;

use super::{FLAT, blank, encode_normal, is_solid, luminance, tilted};
use crate::RgbaImage;

const NONE: u32 = u32::MAX;
/// The dome rises over this many pixels in from the silhouette.
const DOME: f32 = 6.0;
/// Degrees of tilt per step, before Normal Strength scales it.
const TILT_STEP: f32 = 20.0;
const TILT_LEVELS: u8 = 3;
/// A facet smaller than this merges into a neighbour.
const MIN_REGION: u32 = 3;
/// Outlines are colours darker than this.
const OUTLINE_DARK: f32 = 0.3;
const MAX_TILT: f32 = 85.0;

/// What [`generate_normal_map`] is told. Fractions run from 0 to 1 (Normal
/// Strength to 1.5, where 1 is 100%).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct NormalParams {
    /// How strongly brightness and silhouette bend the surface.
    pub shape_strength: f32,
    /// How far normals tilt in the end: the whole map's strength.
    pub normal_strength: f32,
    /// How many pixels in from the silhouette tilt outwards (0 to 8).
    pub edge_width: u32,
    /// 0 keeps each colour cluster a single surface; more splits big
    /// clusters into facets.
    pub facet_splitting: f32,
    /// Thin dark outlines take the normal of the surface beside them.
    pub merge_outlines: bool,
}

impl Default for NormalParams {
    fn default() -> Self {
        Preset::Object.params()
    }
}

/// Starting points for [`NormalParams`], by what the art shows.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Preset {
    /// Large flat faces and hard edges.
    Architecture,
    /// Rounded forms with outlines.
    Character,
    /// A middle of the road start.
    Object,
    /// Broken, bumpy ground with no outlines.
    Terrain,
}

impl Preset {
    pub const ALL: [Preset; 4] =
        [Preset::Architecture, Preset::Character, Preset::Object, Preset::Terrain];

    pub fn name(self) -> &'static str {
        match self {
            Preset::Architecture => "Architecture",
            Preset::Character => "Character",
            Preset::Object => "Object",
            Preset::Terrain => "Terrain",
        }
    }

    pub fn params(self) -> NormalParams {
        let (shape_strength, normal_strength, edge_width, facet_splitting, merge_outlines) =
            match self {
                Preset::Architecture => (0.35, 1.0, 1, 0.15, true),
                Preset::Character => (0.7, 0.9, 1, 0.45, true),
                Preset::Object => (0.55, 1.0, 1, 0.3, true),
                Preset::Terrain => (0.6, 1.1, 0, 0.7, false),
            };
        NormalParams {
            shape_strength,
            normal_strength,
            edge_width,
            facet_splitting,
            merge_outlines,
        }
    }
}

/// What [`generate_light_mask`] is told. All run from 0 to 1.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct MaskParams {
    /// 0 gives every pixel full light; 1 follows the sprite's brightness.
    pub shading_strength: f32,
    /// The least light a pixel receives (the darkest colours).
    pub darkest: f32,
    /// The most light a pixel receives (the lightest colours).
    pub lightest: f32,
    /// Extra darkening of small dark areas enclosed by brighter pixels.
    pub cavity_darkening: f32,
}

impl Default for MaskParams {
    fn default() -> Self {
        MaskParams { shading_strength: 0.8, darkest: 0.35, lightest: 1.0, cavity_darkening: 0.5 }
    }
}

fn n4(i: usize, w: usize, h: usize) -> impl Iterator<Item = usize> {
    let (x, y) = (i % w, i / w);
    [
        (x > 0).then(|| i - 1),
        (x + 1 < w).then(|| i + 1),
        (y > 0).then(|| i - w),
        (y + 1 < h).then(|| i + w),
    ]
    .into_iter()
    .flatten()
}

fn n8(i: usize, w: usize, h: usize) -> impl Iterator<Item = usize> {
    let (x, y) = ((i % w) as i64, (i / w) as i64);
    (-1..=1).flat_map(move |dy| (-1..=1).map(move |dx| (dx, dy))).filter_map(move |(dx, dy)| {
        let (nx, ny) = (x + dx, y + dy);
        let inside = (dx, dy) != (0, 0) && nx >= 0 && ny >= 0 && nx < w as i64 && ny < h as i64;
        inside.then(|| ny as usize * w + nx as usize)
    })
}

fn rgb(image: &RgbaImage, i: usize) -> [u8; 3] {
    [image.rgba[i * 4], image.rgba[i * 4 + 1], image.rgba[i * 4 + 2]]
}

fn lum(image: &RgbaImage, i: usize) -> f32 {
    let [r, g, b] = rgb(image, i);
    luminance(r, g, b)
}

/// Connected pixels of one colour: each pixel's cluster number, and how many.
fn colour_clusters(image: &RgbaImage) -> (Vec<u32>, usize) {
    let (w, h) = (image.width as usize, image.height as usize);
    let mut label = vec![NONE; w * h];
    let mut next = 0u32;
    let mut stack = Vec::new();
    for start in 0..w * h {
        if label[start] != NONE || !is_solid(image, start) {
            continue;
        }
        let colour = rgb(image, start);
        label[start] = next;
        stack.push(start);
        while let Some(i) = stack.pop() {
            for j in n4(i, w, h) {
                if label[j] == NONE && is_solid(image, j) && rgb(image, j) == colour {
                    label[j] = next;
                    stack.push(j);
                }
            }
        }
        next += 1;
    }
    (label, next as usize)
}

/// Steps to the nearest see-through pixel (0 on one). Pixels with none in
/// reach, such as a sprite that fills the image, are very far.
fn distance_to_clear(image: &RgbaImage) -> Vec<u32> {
    let (w, h) = (image.width as usize, image.height as usize);
    const FAR: u32 = 1 << 20;
    let mut d: Vec<u32> = (0..w * h).map(|i| if is_solid(image, i) { FAR } else { 0 }).collect();
    for y in 0..h {
        for x in 0..w {
            let i = y * w + x;
            if x > 0 {
                d[i] = d[i].min(d[i - 1] + 1);
            }
            if y > 0 {
                d[i] = d[i].min(d[i - w] + 1);
            }
        }
    }
    for y in (0..h).rev() {
        for x in (0..w).rev() {
            let i = y * w + x;
            if x + 1 < w {
                d[i] = d[i].min(d[i + 1] + 1);
            }
            if y + 1 < h {
                d[i] = d[i].min(d[i + w] + 1);
            }
        }
    }
    d
}

/// Which pixels are thin dark outlines (see [`super::outline_pixels`]).
pub(crate) fn outline_pixels(image: &RgbaImage) -> Vec<bool> {
    let (w, h) = (image.width as usize, image.height as usize);
    let (label, count) = colour_clusters(image);
    let mut dark = vec![false; count];
    let mut thin = vec![true; count];
    for i in 0..w * h {
        let c = label[i];
        if c == NONE {
            continue;
        }
        dark[c as usize] = lum(image, i) < OUTLINE_DARK;
        let inside = n4(i, w, h).count() == 4 && n4(i, w, h).all(|j| label[j] == c);
        if inside {
            thin[c as usize] = false;
        }
    }
    (0..w * h)
        .map(|i| label[i] != NONE && dark[label[i] as usize] && thin[label[i] as usize])
        .collect()
}

/// A direction and a tilt, quantised: `level * 8 + sector`, with level 0
/// (flat) always 0.
fn quantise(v: (f32, f32)) -> u8 {
    let tilt = v.0.hypot(v.1).atan().to_degrees();
    let level = (tilt / TILT_STEP).round().clamp(0.0, f32::from(TILT_LEVELS)) as u8;
    if level == 0 {
        return 0;
    }
    let sector = ((v.1.atan2(v.0).to_degrees() / 45.0).round() as i32).rem_euclid(8) as u8;
    level * 8 + sector
}

fn key_normal(key: u8, strength: f32) -> [u8; 3] {
    let (level, sector) = (key / 8, key % 8);
    if level == 0 {
        return FLAT;
    }
    let tilt = (f32::from(level) * TILT_STEP * strength).clamp(0.0, MAX_TILT);
    if tilt <= 0.0 { FLAT } else { encode_normal(tilted(f32::from(sector) * 45.0, tilt)) }
}

/// Estimates a normal map for `albedo`. The result is the same size; its
/// alpha is `albedo`'s, and see-through pixels are flat.
pub fn generate_normal_map(albedo: &RgbaImage, params: &NormalParams) -> RgbaImage {
    let (w, h) = (albedo.width as usize, albedo.height as usize);
    let mut out = blank(albedo.width, albedo.height);
    if w == 0 || h == 0 {
        return out;
    }
    let solid: Vec<bool> = (0..w * h).map(|i| is_solid(albedo, i)).collect();
    let (cluster, clusters) = colour_clusters(albedo);
    let dist = distance_to_clear(albedo);

    // The height guess: a dome from the silhouette and the brightness.
    let smooth: Vec<f32> = (0..w * h)
        .map(|i| {
            if !solid[i] {
                return 0.0;
            }
            let (sum, n) = n8(i, w, h)
                .filter(|&j| solid[j])
                .fold((lum(albedo, i), 1.0), |(s, n), j| (s + lum(albedo, j), n + 1.0));
            sum / n
        })
        .collect();
    let height: Vec<f32> = (0..w * h)
        .map(|i| 0.55 * (dist[i].min(DOME as u32) as f32 / DOME) + 0.45 * smooth[i])
        .collect();

    // The slope at each pixel, as the direction its surface faces (image up
    // is positive y).
    let k = 12.0 * params.shape_strength.max(0.0);
    let slope: Vec<(f32, f32)> = (0..w * h)
        .map(|i| {
            if !solid[i] {
                return (0.0, 0.0);
            }
            let (x, y) = (i % w, i / w);
            let at = |j: usize| if solid[j] { height[j] } else { height[i] };
            let left = if x > 0 { at(i - 1) } else { height[i] };
            let right = if x + 1 < w { at(i + 1) } else { height[i] };
            let up = if y > 0 { at(i - w) } else { height[i] };
            let down = if y + 1 < h { at(i + w) } else { height[i] };
            let (gx, gy) = ((right - left) / 2.0, (down - up) / 2.0);
            (-gx * k, gy * k)
        })
        .collect();

    // A cluster's own direction is its average slope.
    let mut sum = vec![(0.0f32, 0.0f32); clusters];
    let mut size = vec![0u32; clusters];
    for i in 0..w * h {
        if cluster[i] != NONE {
            let c = cluster[i] as usize;
            sum[c] = (sum[c].0 + slope[i].0, sum[c].1 + slope[i].1);
            size[c] += 1;
        }
    }
    let cluster_key: Vec<u8> = (0..clusters)
        .map(|c| quantise((sum[c].0 / size[c] as f32, sum[c].1 / size[c] as f32)))
        .collect();
    let split_min = 10.0 * 400f32.powf(1.0 - params.facet_splitting.clamp(0.0, 1.0));
    let splits: Vec<bool> =
        size.iter().map(|&s| params.facet_splitting > 0.0 && s as f32 >= split_min).collect();
    let key: Vec<u8> = (0..w * h)
        .map(|i| match cluster[i] {
            NONE => 0,
            c if splits[c as usize] => quantise(slope[i]),
            c => cluster_key[c as usize],
        })
        .collect();

    // Facets: touching pixels of one cluster and one direction.
    let mut region = vec![NONE; w * h];
    let mut region_key = Vec::new();
    let mut region_size = Vec::new();
    let mut stack = Vec::new();
    for start in 0..w * h {
        if region[start] != NONE || !solid[start] {
            continue;
        }
        let id = region_key.len() as u32;
        region[start] = id;
        stack.push(start);
        let mut count = 0u32;
        while let Some(i) = stack.pop() {
            count += 1;
            for j in n4(i, w, h) {
                if region[j] == NONE && solid[j] && cluster[j] == cluster[i] && key[j] == key[i] {
                    region[j] = id;
                    stack.push(j);
                }
            }
        }
        region_key.push(key[start]);
        region_size.push(count);
    }

    // Tiny facets join the neighbour they share the most border with.
    let mut border: BTreeMap<(u32, u32), u32> = BTreeMap::new();
    for i in 0..w * h {
        if !solid[i] {
            continue;
        }
        for j in [i + 1, i + w] {
            let touching = if j == i + 1 { (i % w) + 1 < w } else { j < w * h };
            if touching && solid[j] && region[i] != region[j] {
                *border.entry((region[i], region[j])).or_default() += 1;
                *border.entry((region[j], region[i])).or_default() += 1;
            }
        }
    }
    for r in 0..region_key.len() as u32 {
        if region_size[r as usize] >= MIN_REGION {
            continue;
        }
        let best = border
            .range((r, 0)..=(r, u32::MAX))
            .map(|(&(_, other), &n)| {
                (region_size[other as usize] >= MIN_REGION, n, std::cmp::Reverse(other))
            })
            .max();
        if let Some((_, _, std::cmp::Reverse(other))) = best {
            region_key[r as usize] = region_key[other as usize];
        }
    }
    let mut key: Vec<u8> =
        (0..w * h).map(|i| if solid[i] { region_key[region[i] as usize] } else { 0 }).collect();

    // Outlines take the surface beside them.
    if params.merge_outlines {
        let outline = outline_pixels(albedo);
        let mut resolved: Vec<bool> = (0..w * h).map(|i| !outline[i]).collect();
        for _ in 0..4 {
            let mut changed = false;
            for i in 0..w * h {
                if resolved[i] || !solid[i] {
                    continue;
                }
                let beside = n4(i, w, h)
                    .chain(n8(i, w, h))
                    .find(|&j| solid[j] && resolved[j] && !outline[j]);
                if let Some(j) = beside {
                    key[i] = key[j];
                    resolved[i] = true;
                    changed = true;
                }
            }
            if !changed {
                break;
            }
        }
    }

    // The silhouette bevels outwards.
    let mut edge_key = vec![None; w * h];
    if params.edge_width > 0 {
        let d = |i: usize| dist[i].min(64) as f32;
        for i in 0..w * h {
            let steps = dist[i];
            if !solid[i] || steps == 0 || steps > params.edge_width {
                continue;
            }
            let (x, y) = (i % w, i / w);
            let at = |j: Option<usize>| j.map_or(d(i), d);
            let left = at((x > 0).then(|| i - 1));
            let right = at((x + 1 < w).then(|| i + 1));
            let up = at((y > 0).then(|| i - w));
            let down = at((y + 1 < h).then(|| i + w));
            // Outwards is where the distance shrinks.
            let (gx, gy) = (right - left, down - up);
            let outward = (-gx, gy);
            if outward == (0.0, 0.0) {
                continue;
            }
            let sector = ((outward.1.atan2(outward.0).to_degrees() / 45.0).round() as i32)
                .rem_euclid(8) as u8;
            let level = if steps == 1 { 2 } else { 1 };
            edge_key[i] = Some(level * 8 + sector);
        }
    }

    for i in 0..w * h {
        let alpha = albedo.rgba[i * 4 + 3];
        let normal = if solid[i] {
            key_normal(edge_key[i].unwrap_or(key[i]), params.normal_strength.max(0.0))
        } else {
            FLAT
        };
        out.rgba[i * 4..i * 4 + 4].copy_from_slice(&[normal[0], normal[1], normal[2], alpha]);
    }
    out
}

/// Makes a light mask for `albedo`: how much direct light each pixel
/// receives, from its brightness. The result is grey, the same size, with
/// `albedo`'s alpha. It does not cast shadows.
pub fn generate_light_mask(albedo: &RgbaImage, params: &MaskParams) -> RgbaImage {
    let (w, h) = (albedo.width as usize, albedo.height as usize);
    let mut out = blank(albedo.width, albedo.height);
    let darkest = params.darkest.clamp(0.0, 1.0);
    let lightest = params.lightest.clamp(darkest, 1.0);
    let strength = params.shading_strength.clamp(0.0, 1.0);
    let mut mask = vec![1.0f32; w * h];
    for (i, m) in mask.iter_mut().enumerate() {
        if is_solid(albedo, i) {
            let value = darkest + lum(albedo, i) * (lightest - darkest);
            *m = 1.0 - strength * (1.0 - value);
        }
    }

    // Small dark areas with brighter pixels all around them are cavities.
    let cavity = params.cavity_darkening.clamp(0.0, 1.0);
    if cavity > 0.0 {
        let dark: Vec<bool> =
            (0..w * h).map(|i| is_solid(albedo, i) && lum(albedo, i) < OUTLINE_DARK).collect();
        let mut seen = vec![false; w * h];
        let mut stack = Vec::new();
        for start in 0..w * h {
            if seen[start] || !dark[start] {
                continue;
            }
            let mut members = Vec::new();
            let mut enclosed = true;
            seen[start] = true;
            stack.push(start);
            while let Some(i) = stack.pop() {
                members.push(i);
                if n4(i, w, h).count() < 4 || n4(i, w, h).any(|j| !is_solid(albedo, j)) {
                    enclosed = false;
                }
                for j in n4(i, w, h) {
                    if dark[j] && !seen[j] {
                        seen[j] = true;
                        stack.push(j);
                    }
                }
            }
            if enclosed && members.len() <= 64 {
                for i in members {
                    mask[i] *= 1.0 - cavity;
                }
            }
        }
    }

    for i in 0..w * h {
        let v = (mask[i].clamp(0.0, 1.0) * 255.0).round() as u8;
        out.rgba[i * 4..i * 4 + 4].copy_from_slice(&[v, v, v, albedo.rgba[i * 4 + 3]]);
    }
    out
}

#[cfg(test)]
mod tests {
    use super::super::decode_normal;
    use super::super::fixtures::{image, pixel};
    use super::*;

    const CLEAR: [u8; 4] = [0, 0, 0, 0];

    /// A flat-coloured disc of `radius` in a `size`-square image.
    fn disc(size: u32, radius: f32, colour: [u8; 4]) -> RgbaImage {
        let c = (size as f32 - 1.0) / 2.0;
        image(size, size, |x, y| {
            let d = ((x as f32 - c).powi(2) + (y as f32 - c).powi(2)).sqrt();
            if d <= radius { colour } else { CLEAR }
        })
    }

    fn normal_at(map: &RgbaImage, x: u32, y: u32) -> [f32; 3] {
        let p = pixel(map, x, y);
        decode_normal([p[0], p[1], p[2]])
    }

    #[test]
    fn a_disc_bevels_outwards_and_keeps_its_alpha() {
        let sprite = disc(17, 7.0, [200, 120, 60, 255]);
        let map = generate_normal_map(&sprite, &NormalParams::default());
        assert_eq!((map.width, map.height), (17, 17));
        // The silhouette tilts away from the middle (up is positive y).
        assert!(normal_at(&map, 1, 8)[0] < -0.2, "left edge faces left");
        assert!(normal_at(&map, 15, 8)[0] > 0.2, "right edge faces right");
        assert!(normal_at(&map, 8, 1)[1] > 0.2, "top edge faces up");
        assert!(normal_at(&map, 8, 15)[1] < -0.2, "bottom edge faces down");
        // The middle and the see-through corners are flat, alpha is kept.
        assert!(normal_at(&map, 8, 8)[2] > 0.99, "the middle faces the viewer");
        assert_eq!(pixel(&map, 0, 0), [FLAT[0], FLAT[1], FLAT[2], 0]);
        assert_eq!(pixel(&map, 8, 8)[3], 255);
    }

    #[test]
    fn the_same_sprite_always_gives_the_same_map() {
        let sprite = disc(15, 6.0, [90, 160, 220, 255]);
        let params = Preset::Character.params();
        assert_eq!(generate_normal_map(&sprite, &params), generate_normal_map(&sprite, &params));
    }

    #[test]
    fn normal_strength_zero_is_flat_and_edge_width_zero_has_no_bevel() {
        let sprite = disc(15, 6.0, [200, 200, 200, 255]);
        let params = NormalParams::default();
        let flat = generate_normal_map(&sprite, &NormalParams { normal_strength: 0.0, ..params });
        assert!(flat.rgba.chunks(4).all(|p| p[..3] == FLAT));
        // Without a bevel or facets, a one-colour disc is one surface.
        let plain = NormalParams { edge_width: 0, facet_splitting: 0.0, ..params };
        let one = generate_normal_map(&sprite, &plain);
        let mut seen: Vec<[u8; 3]> =
            one.rgba.chunks(4).filter(|p| p[3] > 0).map(|p| [p[0], p[1], p[2]]).collect();
        seen.sort();
        seen.dedup();
        assert_eq!(seen.len(), 1);
        // With the bevel on, the rim differs from the middle.
        let bevelled = generate_normal_map(&sprite, &NormalParams { edge_width: 2, ..plain });
        assert_ne!(pixel(&bevelled, 2, 7)[..3], pixel(&bevelled, 7, 7)[..3]);
    }

    #[test]
    fn facet_splitting_breaks_a_big_cluster_into_more_directions() {
        // One colour with a shaded sphere-like silhouette: a large disc.
        let sprite = disc(41, 19.0, [180, 180, 180, 255]);
        let distinct = |splitting: f32| {
            let params = NormalParams {
                facet_splitting: splitting,
                edge_width: 0,
                ..NormalParams::default()
            };
            let map = generate_normal_map(&sprite, &params);
            let mut seen: Vec<[u8; 3]> =
                map.rgba.chunks(4).filter(|p| p[3] > 0).map(|p| [p[0], p[1], p[2]]).collect();
            seen.sort();
            seen.dedup();
            seen.len()
        };
        assert_eq!(distinct(0.0), 1, "one cluster is one surface");
        assert!(distinct(1.0) > 3, "split into facets: {}", distinct(1.0));
    }

    #[test]
    fn a_lone_odd_pixel_takes_the_surface_around_it() {
        let mut sprite = disc(21, 9.0, [120, 140, 160, 255]);
        let i = (10 * 21 + 14) * 4;
        sprite.rgba[i..i + 4].copy_from_slice(&[255, 255, 255, 255]);
        let params =
            NormalParams { edge_width: 0, facet_splitting: 0.0, ..NormalParams::default() };
        let map = generate_normal_map(&sprite, &params);
        assert_eq!(pixel(&map, 14, 10), pixel(&map, 13, 10), "no one-pixel facet");
    }

    #[test]
    fn merged_outlines_face_the_way_the_surface_beside_them_does() {
        // A light disc inside a one-pixel dark ring.
        let mut sprite = disc(21, 8.0, [210, 190, 150, 255]);
        let c = 10.0f32;
        for y in 0..21u32 {
            for x in 0..21u32 {
                let d = ((x as f32 - c).powi(2) + (y as f32 - c).powi(2)).sqrt();
                if (7.5..=8.5).contains(&d) {
                    let i = (y * 21 + x) as usize * 4;
                    sprite.rgba[i..i + 4].copy_from_slice(&[20, 15, 25, 255]);
                }
            }
        }
        let outline = outline_pixels(&sprite);
        assert!(outline[10 * 21 + 2], "the ring is an outline");
        assert!(!outline[10 * 21 + 10], "the middle is not");
        let merged = generate_normal_map(
            &sprite,
            &NormalParams { merge_outlines: true, edge_width: 0, ..NormalParams::default() },
        );
        // The ring's left side takes the interior's normal just inside it.
        assert_eq!(pixel(&merged, 2, 10)[..3], pixel(&merged, 3, 10)[..3]);
    }

    #[test]
    fn presets_are_valid_and_different() {
        let all: Vec<NormalParams> = Preset::ALL.iter().map(|p| p.params()).collect();
        for p in &all {
            assert!((0.0..=1.0).contains(&p.shape_strength));
            assert!((0.0..=1.5).contains(&p.normal_strength));
            assert!(p.edge_width <= 8);
            assert!((0.0..=1.0).contains(&p.facet_splitting));
        }
        assert!(all.iter().enumerate().all(|(i, a)| all.iter().skip(i + 1).all(|b| a != b)));
        assert_eq!(Preset::Terrain.name(), "Terrain");
    }

    #[test]
    fn the_mask_follows_brightness_between_darkest_and_lightest() {
        let sprite = image(3, 1, |x, _| match x {
            0 => [255, 255, 255, 255],
            1 => [128, 128, 128, 255],
            _ => [0, 0, 0, 255],
        });
        let params = MaskParams {
            shading_strength: 1.0,
            darkest: 0.2,
            lightest: 0.9,
            cavity_darkening: 0.0,
        };
        let mask = generate_light_mask(&sprite, &params);
        let [white, grey, black] = [0, 1, 2].map(|x| pixel(&mask, x, 0));
        assert_eq!(white[0], (0.9f32 * 255.0).round() as u8);
        assert_eq!(black[0], (0.2f32 * 255.0).round() as u8);
        assert!(black[0] < grey[0] && grey[0] < white[0]);
        assert_eq!(white[0], white[1], "grey");
        assert_eq!(white[1], white[2]);
        assert_eq!(white[3], 255);
    }

    #[test]
    fn zero_shading_strength_lets_all_light_in_and_alpha_is_kept() {
        let sprite = image(2, 1, |x, _| if x == 0 { [10, 10, 10, 255] } else { [10, 10, 10, 90] });
        let mask = generate_light_mask(
            &sprite,
            &MaskParams { shading_strength: 0.0, ..MaskParams::default() },
        );
        assert_eq!(pixel(&mask, 0, 0), [255, 255, 255, 255]);
        assert_eq!(pixel(&mask, 1, 0)[3], 90, "alpha comes from the sprite");
    }

    #[test]
    fn enclosed_dark_cavities_are_darkened_more_than_dark_edges() {
        // A light block with a dark dot inside it, and a dark dot at its edge.
        let sprite = image(9, 5, |x, y| match (x, y) {
            (2, 2) => [10, 10, 10, 255],
            (8, 2) => [10, 10, 10, 255],
            (0..=8, 0..=4) => [230, 230, 230, 255],
            _ => CLEAR,
        });
        let params = MaskParams {
            shading_strength: 1.0,
            darkest: 0.5,
            lightest: 1.0,
            cavity_darkening: 0.6,
        };
        let mask = generate_light_mask(&sprite, &params);
        let inside = pixel(&mask, 2, 2)[0];
        let edge = pixel(&mask, 8, 2)[0];
        assert!(inside < edge, "{inside} should be darker than {edge}");
        let none = generate_light_mask(&sprite, &MaskParams { cavity_darkening: 0.0, ..params });
        assert_eq!(pixel(&none, 2, 2)[0], edge, "without cavity darkening they match");
    }
}
