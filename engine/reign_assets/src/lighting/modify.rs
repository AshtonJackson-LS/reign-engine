//! Editing maps by hand: pick pixels with a [`Selection`] (wand, pencil,
//! marquee), then point them somewhere ([`apply_normal`],
//! [`average_normals`]), scale the whole map ([`scale_normal_strength`]) or
//! change a light mask's brightness ([`adjust_brightness`]).

use super::{SOLID_ALPHA, decode_normal, encode_normal, is_solid, outline_pixels, tilted};
use crate::RgbaImage;

/// How a new selection combines with the one already there.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SelectMode {
    /// Throw the old selection away.
    Replace,
    /// Shift+click: keep the old and add to it.
    Add,
    /// Alt+click: take these pixels out of it.
    Subtract,
}

/// A set of pixels of one image.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Selection {
    width: u32,
    height: u32,
    bits: Vec<bool>,
}

impl Selection {
    /// Nothing selected, in an image `width` x `height`.
    pub fn new(width: u32, height: u32) -> Self {
        Selection { width, height, bits: vec![false; width as usize * height as usize] }
    }

    pub fn size(&self) -> (u32, u32) {
        (self.width, self.height)
    }

    pub fn is_empty(&self) -> bool {
        !self.bits.iter().any(|&b| b)
    }

    pub fn count(&self) -> usize {
        self.bits.iter().filter(|&&b| b).count()
    }

    pub fn contains(&self, x: u32, y: u32) -> bool {
        x < self.width && y < self.height && self.bits[(y * self.width + x) as usize]
    }

    pub fn clear(&mut self) {
        self.bits.fill(false);
    }

    /// The selected pixels' positions in row order, as `y * width + x`.
    pub fn pixels(&self) -> impl Iterator<Item = usize> + '_ {
        self.bits.iter().enumerate().filter(|(_, b)| **b).map(|(i, _)| i)
    }

    fn commit(&mut self, shape: &[usize], mode: SelectMode) {
        if mode == SelectMode::Replace {
            self.clear();
        }
        for &i in shape {
            self.bits[i] = mode != SelectMode::Subtract;
        }
    }

    /// The pencil: one pixel. Drag it with [`SelectMode::Add`].
    pub fn select_pixel(&mut self, x: u32, y: u32, mode: SelectMode) {
        let shape = if x < self.width && y < self.height {
            vec![(y * self.width + x) as usize]
        } else {
            Vec::new()
        };
        self.commit(&shape, mode);
    }

    /// The marquee: the rectangle between two corners, both included, in
    /// either order. Parts outside the image are left out.
    pub fn select_rect(&mut self, a: (u32, u32), b: (u32, u32), mode: SelectMode) {
        let (x0, x1) = (a.0.min(b.0), a.0.max(b.0).min(self.width.saturating_sub(1)));
        let (y0, y1) = (a.1.min(b.1), a.1.max(b.1).min(self.height.saturating_sub(1)));
        let mut shape = Vec::new();
        if self.width > 0 && self.height > 0 {
            for y in y0..=y1 {
                for x in x0..=x1 {
                    shape.push((y * self.width + x) as usize);
                }
            }
        }
        self.commit(&shape, mode);
    }

    /// The wand: the pixels that match the colour at `(x, y)` in `source`
    /// (the albedo, or either map). With `contiguous` only those touching
    /// it; without, every pixel of that colour. Clicking see-through pixels
    /// does nothing.
    pub fn wand(&mut self, source: &RgbaImage, x: u32, y: u32, contiguous: bool, mode: SelectMode) {
        if (source.width, source.height) != (self.width, self.height)
            || x >= self.width
            || y >= self.height
        {
            return;
        }
        let (w, h) = (self.width as usize, self.height as usize);
        let start = y as usize * w + x as usize;
        if !is_solid(source, start) {
            return;
        }
        let colour = &source.rgba[start * 4..start * 4 + 4];
        let same = |i: usize| source.rgba[i * 4..i * 4 + 4] == *colour;
        let mut shape = Vec::new();
        if contiguous {
            let mut seen = vec![false; w * h];
            let mut stack = vec![start];
            seen[start] = true;
            while let Some(i) = stack.pop() {
                shape.push(i);
                let (cx, cy) = (i % w, i / w);
                let around = [
                    (cx > 0).then(|| i - 1),
                    (cx + 1 < w).then(|| i + 1),
                    (cy > 0).then(|| i - w),
                    (cy + 1 < h).then(|| i + w),
                ];
                for j in around.into_iter().flatten() {
                    if !seen[j] && same(j) {
                        seen[j] = true;
                        stack.push(j);
                    }
                }
            }
        } else {
            shape.extend((0..w * h).filter(|&i| same(i)));
        }
        self.commit(&shape, mode);
    }

    /// Fills gaps of up to `max_hole` unselected pixels that the selection
    /// completely encloses.
    pub fn fill_holes(&mut self, max_hole: usize) {
        let (w, h) = (self.width as usize, self.height as usize);
        let mut seen = vec![false; w * h];
        let mut fill = Vec::new();
        for start in 0..w * h {
            if seen[start] || self.bits[start] {
                continue;
            }
            let mut members = Vec::new();
            let mut open = false;
            let mut stack = vec![start];
            seen[start] = true;
            while let Some(i) = stack.pop() {
                members.push(i);
                let (x, y) = (i % w, i / w);
                if x == 0 || y == 0 || x + 1 == w || y + 1 == h {
                    open = true;
                }
                let around = [
                    (x > 0).then(|| i - 1),
                    (x + 1 < w).then(|| i + 1),
                    (y > 0).then(|| i - w),
                    (y + 1 < h).then(|| i + w),
                ];
                for j in around.into_iter().flatten() {
                    if !seen[j] && !self.bits[j] {
                        seen[j] = true;
                        stack.push(j);
                    }
                }
            }
            if !open && members.len() <= max_hole {
                fill.extend(members);
            }
        }
        self.commit(&fill, SelectMode::Add);
    }

    /// Adds the thin dark outline pixels of `albedo` that touch the
    /// selection, so an outline goes with the surface it borders.
    pub fn merge_outlines(&mut self, albedo: &RgbaImage) {
        if (albedo.width, albedo.height) != (self.width, self.height) {
            return;
        }
        let (w, h) = (self.width as usize, self.height as usize);
        let outline = outline_pixels(albedo);
        let mut add = Vec::new();
        for i in 0..w * h {
            if !outline[i] || self.bits[i] {
                continue;
            }
            let (x, y) = (i % w, i / w);
            let around = [
                (x > 0).then(|| i - 1),
                (x + 1 < w).then(|| i + 1),
                (y > 0).then(|| i - w),
                (y + 1 < h).then(|| i + w),
            ];
            if around.into_iter().flatten().any(|j| self.bits[j]) {
                add.push(i);
            }
        }
        self.commit(&add, SelectMode::Add);
    }
}

/// The nine directions of "Apply Direction".
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Direction {
    Front,
    Left,
    Right,
    Up,
    Down,
    UpLeft,
    UpRight,
    DownLeft,
    DownRight,
}

impl Direction {
    pub const ALL: [Direction; 9] = [
        Direction::UpLeft,
        Direction::Up,
        Direction::UpRight,
        Direction::Left,
        Direction::Front,
        Direction::Right,
        Direction::DownLeft,
        Direction::Down,
        Direction::DownRight,
    ];

    pub fn label(self) -> &'static str {
        match self {
            Direction::Front => "Front",
            Direction::Left => "Left",
            Direction::Right => "Right",
            Direction::Up => "Up",
            Direction::Down => "Down",
            Direction::UpLeft => "Up left",
            Direction::UpRight => "Up right",
            Direction::DownLeft => "Down left",
            Direction::DownRight => "Down right",
        }
    }

    /// Which way it points: 0 degrees is right, 90 is up. Front has none.
    pub fn heading(self) -> Option<f32> {
        Some(match self {
            Direction::Front => return None,
            Direction::Right => 0.0,
            Direction::UpRight => 45.0,
            Direction::Up => 90.0,
            Direction::UpLeft => 135.0,
            Direction::Left => 180.0,
            Direction::DownLeft => 225.0,
            Direction::Down => 270.0,
            Direction::DownRight => 315.0,
        })
    }
}

/// The normal for `direction`, tilted `angle` degrees from facing the
/// viewer (Front ignores it).
pub fn direction_normal(direction: Direction, angle: f32) -> [f32; 3] {
    match direction.heading() {
        Some(heading) => tilted(heading, angle.clamp(0.0, 89.0)),
        None => [0.0, 0.0, 1.0],
    }
}

/// The facet of the direction sphere at `(x, y)`, in a unit circle with up
/// as positive y. The sphere is cut into a flat middle and three rings of
/// eight facets (20, 40 and 60 degrees). `None` outside the circle.
pub fn sphere_facet(x: f32, y: f32) -> Option<[f32; 3]> {
    let r = x.hypot(y);
    if r > 1.0 {
        return None;
    }
    if r < 0.25 {
        return Some([0.0, 0.0, 1.0]);
    }
    let ring = (((r - 0.25) / 0.75 * 3.0).floor() as i32).clamp(0, 2);
    let sector = ((y.atan2(x).to_degrees() / 45.0).round() as i32).rem_euclid(8);
    Some(tilted(sector as f32 * 45.0, (ring + 1) as f32 * 20.0))
}

fn grey(v: u8) -> [u8; 3] {
    [v, v, v]
}

/// Points every selected pixel of the normal map `map` along `normal`.
/// Alpha is kept, and see-through pixels are left alone. Returns how many
/// pixels changed.
pub fn apply_normal(map: &mut RgbaImage, selection: &Selection, normal: [f32; 3]) -> usize {
    let bytes = encode_normal(normal);
    let mut changed = 0;
    for i in selection.pixels() {
        if i * 4 + 3 < map.rgba.len() && map.rgba[i * 4 + 3] >= SOLID_ALPHA {
            map.rgba[i * 4..i * 4 + 3].copy_from_slice(&bytes);
            changed += 1;
        }
    }
    changed
}

/// Gives every selected pixel the average of the normals already there.
/// Returns false if nothing was selected.
pub fn average_normals(map: &mut RgbaImage, selection: &Selection) -> bool {
    let mut sum = [0.0f32; 3];
    let mut count = 0;
    for i in selection.pixels() {
        if i * 4 + 3 < map.rgba.len() && map.rgba[i * 4 + 3] >= SOLID_ALPHA {
            let n = decode_normal([map.rgba[i * 4], map.rgba[i * 4 + 1], map.rgba[i * 4 + 2]]);
            for k in 0..3 {
                sum[k] += n[k];
            }
            count += 1;
        }
    }
    if count == 0 {
        return false;
    }
    let len = (sum[0] * sum[0] + sum[1] * sum[1] + sum[2] * sum[2]).sqrt();
    let average = if len < 1e-6 { [0.0, 0.0, 1.0] } else { sum.map(|c| c / len) };
    apply_normal(map, selection, average);
    true
}

/// Scales how far every normal in the map tilts, by `factor` (1 leaves it,
/// 0.5 halves the tilt, 0 flattens it). Tilts stop short of 85 degrees.
pub fn scale_normal_strength(map: &mut RgbaImage, factor: f32) {
    for pixel in map.rgba.as_chunks_mut::<4>().0 {
        if pixel[3] < SOLID_ALPHA {
            continue;
        }
        let n = decode_normal([pixel[0], pixel[1], pixel[2]]);
        let tilt = n[2].clamp(-1.0, 1.0).acos().to_degrees();
        if tilt < 0.5 {
            continue;
        }
        let heading = n[1].atan2(n[0]).to_degrees();
        let scaled = (tilt * factor.max(0.0)).clamp(0.0, 85.0);
        let bytes = encode_normal(tilted(heading, scaled));
        pixel[..3].copy_from_slice(&bytes);
    }
}

/// How [`adjust_brightness`] reads its percentage.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum BrightnessMode {
    /// Multiplies what is there: 50% halves each pixel.
    Relative,
    /// Sets every pixel to that value.
    Fixed,
}

/// Changes the light mask's value under the selection. Alpha is kept and
/// see-through pixels are left alone.
pub fn adjust_brightness(
    mask: &mut RgbaImage,
    selection: &Selection,
    percent: f32,
    mode: BrightnessMode,
) {
    for i in selection.pixels() {
        if i * 4 + 3 >= mask.rgba.len() || mask.rgba[i * 4 + 3] < SOLID_ALPHA {
            continue;
        }
        let was = f32::from(mask.rgba[i * 4]) / 255.0;
        let now = match mode {
            BrightnessMode::Relative => was * percent / 100.0,
            BrightnessMode::Fixed => percent / 100.0,
        };
        let v = (now.clamp(0.0, 1.0) * 255.0).round() as u8;
        mask.rgba[i * 4..i * 4 + 3].copy_from_slice(&grey(v));
    }
}

#[cfg(test)]
mod tests {
    use super::super::FLAT;
    use super::super::fixtures::{image, pixel};
    use super::*;

    const A: [u8; 4] = [200, 50, 50, 255];
    const B: [u8; 4] = [50, 200, 50, 255];

    /// Two red patches split by a green column.
    fn striped() -> RgbaImage {
        image(5, 3, |x, _| if x == 2 { B } else { A })
    }

    #[test]
    fn the_wand_takes_a_touching_cluster_or_every_pixel_of_the_colour() {
        let img = striped();
        let mut sel = Selection::new(5, 3);
        sel.wand(&img, 0, 0, true, SelectMode::Replace);
        assert_eq!(sel.count(), 6, "the left patch only");
        assert!(sel.contains(1, 2) && !sel.contains(3, 0));
        sel.wand(&img, 0, 0, false, SelectMode::Replace);
        assert_eq!(sel.count(), 12, "both patches: same colour, not touching");
    }

    #[test]
    fn shift_adds_alt_subtracts_and_replace_starts_over() {
        let img = striped();
        let mut sel = Selection::new(5, 3);
        sel.wand(&img, 0, 0, true, SelectMode::Replace);
        sel.wand(&img, 2, 0, true, SelectMode::Add);
        assert_eq!(sel.count(), 9);
        sel.wand(&img, 0, 0, true, SelectMode::Subtract);
        assert_eq!(sel.count(), 3);
        sel.wand(&img, 4, 1, true, SelectMode::Replace);
        assert_eq!(sel.count(), 6);
        assert!(!sel.contains(2, 1));
        sel.clear();
        assert!(sel.is_empty());
    }

    #[test]
    fn clicking_clear_pixels_or_a_different_size_does_nothing() {
        let img = image(2, 2, |x, _| if x == 0 { [9, 9, 9, 0] } else { A });
        let mut sel = Selection::new(2, 2);
        sel.wand(&img, 0, 0, true, SelectMode::Replace);
        assert!(sel.is_empty());
        let mut other = Selection::new(3, 3);
        other.wand(&img, 1, 0, true, SelectMode::Replace);
        assert!(other.is_empty());
    }

    #[test]
    fn pencil_and_marquee_select_what_they_cover() {
        let mut sel = Selection::new(6, 4);
        sel.select_pixel(1, 1, SelectMode::Add);
        sel.select_pixel(2, 1, SelectMode::Add);
        assert_eq!(sel.count(), 2);
        sel.select_rect((4, 3), (2, 1), SelectMode::Replace);
        assert_eq!(sel.count(), 9, "corners in either order, both included");
        sel.select_rect((0, 0), (99, 99), SelectMode::Subtract);
        assert!(sel.is_empty(), "clamped to the image");
        sel.select_pixel(50, 50, SelectMode::Add);
        assert!(sel.is_empty());
    }

    #[test]
    fn fill_holes_fills_only_small_enclosed_gaps() {
        let mut sel = Selection::new(7, 7);
        sel.select_rect((1, 1), (5, 5), SelectMode::Replace);
        sel.select_pixel(3, 3, SelectMode::Subtract);
        assert_eq!(sel.count(), 24);
        sel.fill_holes(8);
        assert_eq!(sel.count(), 25, "the one-pixel hole is filled");
        // A hole that touches the edge isn't enclosed; a big one is left.
        let mut open = Selection::new(5, 5);
        open.select_rect((0, 0), (4, 4), SelectMode::Replace);
        open.select_pixel(0, 2, SelectMode::Subtract);
        open.fill_holes(8);
        assert_eq!(open.count(), 24, "it touches the border");
        let mut big = Selection::new(9, 9);
        big.select_rect((0, 0), (8, 8), SelectMode::Replace);
        big.select_rect((2, 2), (6, 6), SelectMode::Subtract);
        big.fill_holes(8);
        assert_eq!(big.count(), 81 - 25, "25 pixels is more than 8");
    }

    #[test]
    fn merging_outlines_adds_the_outline_touching_the_selection() {
        // A light surface (columns 2-5) with a dark outline column on each side.
        let img = image(8, 3, |x, _| match x {
            1 | 6 => [10, 10, 10, 255],
            2..=5 => [220, 200, 160, 255],
            _ => [0, 0, 0, 0],
        });
        let mut sel = Selection::new(8, 3);
        sel.wand(&img, 3, 1, true, SelectMode::Replace);
        assert_eq!(sel.count(), 12);
        sel.merge_outlines(&img);
        assert_eq!(sel.count(), 18, "both outline columns join");
        assert!(sel.contains(1, 0) && sel.contains(6, 2));
    }

    #[test]
    fn directions_point_the_way_they_say() {
        let up = direction_normal(Direction::Up, 45.0);
        assert!(up[1] > 0.7 && up[0].abs() < 1e-5);
        let left = direction_normal(Direction::Left, 45.0);
        assert!(left[0] < -0.7);
        let down_right = direction_normal(Direction::DownRight, 30.0);
        assert!(down_right[0] > 0.0 && down_right[1] < 0.0);
        assert_eq!(direction_normal(Direction::Front, 45.0), [0.0, 0.0, 1.0]);
        assert_eq!(Direction::ALL.len(), 9);
        assert_eq!(Direction::UpLeft.label(), "Up left");
    }

    #[test]
    fn the_sphere_has_a_flat_middle_rings_of_facets_and_an_outside() {
        assert_eq!(sphere_facet(0.0, 0.0), Some([0.0, 0.0, 1.0]));
        assert_eq!(sphere_facet(1.0, 1.0), None);
        let inner = sphere_facet(0.4, 0.0).unwrap();
        let outer = sphere_facet(0.95, 0.0).unwrap();
        assert!(outer[2] < inner[2], "further out tilts more");
        assert!(inner[0] > 0.0 && outer[0] > 0.0, "to the right");
        assert_eq!(sphere_facet(0.5, 0.01), sphere_facet(0.5, -0.01), "the same facet");
    }

    fn flat_map(w: u32, h: u32) -> RgbaImage {
        image(w, h, |_, _| [FLAT[0], FLAT[1], FLAT[2], 255])
    }

    #[test]
    fn applying_a_normal_changes_only_the_selected_solid_pixels() {
        let mut map = flat_map(3, 1);
        map.rgba[2 * 4 + 3] = 0;
        let mut sel = Selection::new(3, 1);
        sel.select_rect((1, 0), (2, 0), SelectMode::Replace);
        let n = direction_normal(Direction::Right, 45.0);
        assert_eq!(apply_normal(&mut map, &sel, n), 1, "the clear pixel is left alone");
        assert_eq!(pixel(&map, 0, 0)[..3], FLAT);
        assert!(pixel(&map, 1, 0)[0] > 200);
        assert_eq!(pixel(&map, 2, 0)[..3], FLAT);
    }

    #[test]
    fn averaging_blends_the_selected_normals() {
        let mut map = flat_map(2, 1);
        let left = encode_normal(direction_normal(Direction::Left, 40.0));
        let right = encode_normal(direction_normal(Direction::Right, 40.0));
        map.rgba[0..3].copy_from_slice(&left);
        map.rgba[4..7].copy_from_slice(&right);
        let mut sel = Selection::new(2, 1);
        sel.select_rect((0, 0), (1, 0), SelectMode::Replace);
        assert!(average_normals(&mut map, &sel));
        let n = decode_normal([map.rgba[0], map.rgba[1], map.rgba[2]]);
        assert!(n[0].abs() < 0.03 && n[2] > 0.99, "left and right cancel: {n:?}");
        assert_eq!(map.rgba[0..3], map.rgba[4..7]);
        assert!(!average_normals(&mut map, &Selection::new(2, 1)), "nothing selected");
    }

    #[test]
    fn scaling_strength_scales_tilt_and_leaves_flat_alone() {
        let mut map = flat_map(2, 1);
        map.rgba[0..3].copy_from_slice(&encode_normal(direction_normal(Direction::Right, 40.0)));
        let tilt = |m: &RgbaImage, x: u32| {
            let p = pixel(m, x, 0);
            decode_normal([p[0], p[1], p[2]])[2].acos().to_degrees()
        };
        let before = tilt(&map, 0);
        scale_normal_strength(&mut map, 0.5);
        assert!((tilt(&map, 0) - before / 2.0).abs() < 1.5, "halved: {}", tilt(&map, 0));
        assert_eq!(pixel(&map, 1, 0)[..3], FLAT);
        scale_normal_strength(&mut map, 0.0);
        assert_eq!(pixel(&map, 0, 0)[..3], FLAT, "zero flattens");
    }

    #[test]
    fn brightness_is_relative_or_fixed() {
        let mut mask =
            image(2, 1, |x, _| if x == 0 { [200, 200, 200, 255] } else { [100, 100, 100, 255] });
        let mut sel = Selection::new(2, 1);
        sel.select_pixel(0, 0, SelectMode::Add);
        adjust_brightness(&mut mask, &sel, 50.0, BrightnessMode::Relative);
        assert_eq!(pixel(&mask, 0, 0), [100, 100, 100, 255]);
        assert_eq!(pixel(&mask, 1, 0), [100, 100, 100, 255], "not selected");
        adjust_brightness(&mut mask, &sel, 100.0, BrightnessMode::Fixed);
        assert_eq!(pixel(&mask, 0, 0), [255, 255, 255, 255]);
        adjust_brightness(&mut mask, &sel, 250.0, BrightnessMode::Relative);
        assert_eq!(pixel(&mask, 0, 0)[0], 255, "clamped");
    }
}
