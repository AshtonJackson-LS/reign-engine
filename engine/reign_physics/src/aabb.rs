/// An axis-aligned box in whole world pixels, origin top-left. Used for
/// hitboxes and hurtboxes, which overlap but never push each other.
#[derive(
    Clone,
    Copy,
    Debug,
    Default,
    PartialEq,
    Eq,
    Hash,
    serde::Serialize,
    serde::Deserialize,
    reign_scene::Reflect,
)]
pub struct Aabb {
    pub x: i32,
    pub y: i32,
    pub w: i32,
    pub h: i32,
}

impl Aabb {
    pub const fn new(x: i32, y: i32, w: i32, h: i32) -> Self {
        Aabb { x, y, w, h }
    }

    /// True if the boxes share at least one pixel. Touching edges don't count.
    pub fn overlaps(&self, other: &Aabb) -> bool {
        self.x < other.x + other.w
            && other.x < self.x + self.w
            && self.y < other.y + other.h
            && other.y < self.y + self.h
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn overlap_needs_a_shared_pixel() {
        let a = Aabb::new(0, 0, 10, 10);
        assert!(a.overlaps(&Aabb::new(9, 9, 5, 5)));
        assert!(!a.overlaps(&Aabb::new(10, 0, 5, 5)), "touching edges");
        assert!(!a.overlaps(&Aabb::new(0, -5, 5, 5)));
    }
}
