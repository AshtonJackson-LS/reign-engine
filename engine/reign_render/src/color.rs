/// An sRGB color as artists pick it, converted to linear for the GPU.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Color {
    pub r: f32,
    pub g: f32,
    pub b: f32,
    pub a: f32,
}

impl Color {
    pub const BLACK: Color = Color::rgb8(0, 0, 0);
    pub const WHITE: Color = Color::rgb8(255, 255, 255);

    pub const fn rgb8(r: u8, g: u8, b: u8) -> Self {
        Color::rgba8(r, g, b, 255)
    }

    pub const fn rgba8(r: u8, g: u8, b: u8, a: u8) -> Self {
        Color { r: r as f32 / 255.0, g: g as f32 / 255.0, b: b as f32 / 255.0, a: a as f32 / 255.0 }
    }

    pub fn to_linear(self) -> [f32; 4] {
        fn channel(c: f32) -> f32 {
            if c <= 0.04045 { c / 12.92 } else { ((c + 0.055) / 1.055).powf(2.4) }
        }
        [channel(self.r), channel(self.g), channel(self.b), self.a]
    }
}
