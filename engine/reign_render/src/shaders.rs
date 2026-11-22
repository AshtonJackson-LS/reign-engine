//! WGSL sources. Kept inline until the asset pipeline can hot reload them.

pub const RECT: &str = r#"
// World view in world pixels; must match VIEW_WIDTH/VIEW_HEIGHT.
const VIEW: vec2<f32> = vec2<f32>(640.0, 360.0);

struct Rect {
    @location(0) pos: vec2<f32>,
    @location(1) edge_x: vec2<f32>,
    @location(2) edge_y: vec2<f32>,
    @location(3) color: vec4<f32>,
};

struct VsOut {
    @builtin(position) clip: vec4<f32>,
    @location(0) color: vec4<f32>,
};

@vertex
fn vs_main(@builtin(vertex_index) i: u32, rect: Rect) -> VsOut {
    // Two triangles: 0-1-2, 2-1-3 over corners (0,0) (1,0) (0,1) (1,1).
    var corners = array<vec2<f32>, 6>(
        vec2(0.0, 0.0), vec2(1.0, 0.0), vec2(0.0, 1.0),
        vec2(0.0, 1.0), vec2(1.0, 0.0), vec2(1.0, 1.0),
    );
    let c = corners[i];
    let px = rect.pos + c.x * rect.edge_x + c.y * rect.edge_y;
    let ndc = px / VIEW * 2.0 - 1.0;
    var out: VsOut;
    out.clip = vec4(ndc.x, -ndc.y, 0.0, 1.0);
    out.color = rect.color;
    return out;
}

@fragment
fn fs_main(in: VsOut) -> @location(0) vec4<f32> {
    return in.color;
}
"#;

pub const SPRITE: &str = r#"
// World view in world pixels; must match VIEW_WIDTH/VIEW_HEIGHT.
const VIEW: vec2<f32> = vec2<f32>(640.0, 360.0);

@group(0) @binding(0) var sheet: texture_2d<f32>;
@group(0) @binding(1) var pixel_sampler: sampler;

struct Sprite {
    @location(0) pos: vec2<f32>,
    @location(1) size: vec2<f32>,
    @location(2) uv_min: vec2<f32>,
    @location(3) uv_max: vec2<f32>,
    @location(4) color: vec4<f32>,
    @location(5) crimson: vec2<f32>,
};

struct VsOut {
    @builtin(position) clip: vec4<f32>,
    @location(0) uv: vec2<f32>,
    @location(1) color: vec4<f32>,
    @location(2) local: vec2<f32>,
    @location(3) @interpolate(flat) crimson: vec2<f32>,
    @location(4) @interpolate(flat) bounds: vec4<f32>,
};

@vertex
fn vs_main(@builtin(vertex_index) i: u32, s: Sprite) -> VsOut {
    var corners = array<vec2<f32>, 6>(
        vec2(0.0, 0.0), vec2(1.0, 0.0), vec2(0.0, 1.0),
        vec2(0.0, 1.0), vec2(1.0, 0.0), vec2(1.0, 1.0),
    );
    let c = corners[i];
    let ndc = (s.pos + c * s.size) / VIEW * 2.0 - 1.0;
    var out: VsOut;
    out.clip = vec4(ndc.x, -ndc.y, 0.0, 1.0);
    out.uv = mix(s.uv_min, s.uv_max, c);
    out.color = s.color;
    out.local = c;
    out.crimson = s.crimson;
    out.bounds = vec4(min(s.uv_min, s.uv_max), max(s.uv_min, s.uv_max));
    return out;
}

@fragment
fn fs_main(in: VsOut) -> @location(0) vec4<f32> {
    let base = textureSample(sheet, pixel_sampler, in.uv) * in.color;
    if in.crimson.y <= 0.0 {
        return base;
    }
    let texel = 1.0 / vec2<f32>(textureDimensions(sheet));
    let lo = in.bounds.xy + texel * 0.5;
    let hi = in.bounds.zw - texel * 0.5;
    // Two source texels form a crisp one-world-pixel inner rim. Clamp
    // samples to this animation cell so neighboring frames cannot bleed.
    let left = textureSampleLevel(sheet, pixel_sampler, clamp(in.uv - vec2(texel.x * 2.0, 0.0), lo, hi), 0.0).a;
    let right = textureSampleLevel(sheet, pixel_sampler, clamp(in.uv + vec2(texel.x * 2.0, 0.0), lo, hi), 0.0).a;
    let up = textureSampleLevel(sheet, pixel_sampler, clamp(in.uv - vec2(0.0, texel.y * 2.0), lo, hi), 0.0).a;
    let down = textureSampleLevel(sheet, pixel_sampler, clamp(in.uv + vec2(0.0, texel.y * 2.0), lo, hi), 0.0).a;
    let rim = 1.0 - min(min(left, right), min(up, down));
    let time = in.crimson.x;
    let warp = sin(in.local.x * 19.0 + time * 2.0) * 0.65;
    let flow = sin(in.local.y * 38.0 + in.local.x * 9.0 + time * 4.0 + warp);
    let ribbon = pow(0.5 + flow * 0.5, 10.0);
    let ember = vec3<f32>(1.15, 0.025, 0.045);
    let core = vec3<f32>(1.25, 0.16, 0.12);
    let energy = ember * (0.035 + ribbon * 0.32) + core * rim * 0.6;
    // Keep the original silhouette and damage/invulnerability opacity.
    return vec4(base.rgb + energy * in.crimson.y, base.a);
}
"#;

/// A point light in the light map: a soft disc, brightest in the middle.
///
/// Where the material target says a pixel has maps (alpha 1), the light is
/// shaded by the pixel's normal and scaled by its light mask; elsewhere it is
/// the plain radial light it always was. The material target is the same size
/// as the light map and holds, per pixel, the normal's x and y in red and
/// green (0.5 is zero), the mask in blue, and 1 in alpha for pixels with maps.
/// `reign_assets::lighting::shade_pixel` is the same formula, for the editor.
pub const LIGHT: &str = r#"
const VIEW: vec2<f32> = vec2<f32>(640.0, 360.0);

@group(0) @binding(0) var material: texture_2d<f32>;
@group(0) @binding(1) var material_sampler: sampler;

struct Light {
    @location(0) pos: vec2<f32>,
    @location(1) radius: f32,
    @location(2) height: f32,
    @location(3) color: vec4<f32>,
};

struct VsOut {
    @builtin(position) clip: vec4<f32>,
    @location(0) local: vec2<f32>,
    @location(1) color: vec4<f32>,
    @location(2) world: vec2<f32>,
    @location(3) @interpolate(flat) centre: vec2<f32>,
    @location(4) @interpolate(flat) height: f32,
};

@vertex
fn vs_main(@builtin(vertex_index) i: u32, l: Light) -> VsOut {
    var corners = array<vec2<f32>, 6>(
        vec2(-1.0, -1.0), vec2(1.0, -1.0), vec2(-1.0, 1.0),
        vec2(-1.0, 1.0), vec2(1.0, -1.0), vec2(1.0, 1.0),
    );
    let c = corners[i];
    let world = l.pos + c * l.radius;
    let ndc = world / VIEW * 2.0 - 1.0;
    var out: VsOut;
    out.clip = vec4(ndc.x, -ndc.y, 0.0, 1.0);
    out.local = c;
    out.color = l.color;
    out.world = world;
    out.centre = l.pos;
    out.height = l.height;
    return out;
}

@fragment
fn fs_main(in: VsOut) -> @location(0) vec4<f32> {
    let d2 = dot(in.local, in.local);
    // Smooth to zero at the radius, with a hot core like a real falloff.
    let edge = clamp(1.0 - d2, 0.0, 1.0);
    let f = edge * edge / (1.0 + 8.0 * d2);
    let m = textureSampleLevel(material, material_sampler, in.world / VIEW, 0.0);
    var shade = 1.0;
    if m.a > 0.5 {
        let xy = m.rg * 2.0 - 1.0;
        let n = vec3(xy, sqrt(max(1.0 - dot(xy, xy), 0.0)));
        // Image y grows down, normals have up as positive.
        let to_light = in.centre - in.world;
        let v = vec3(to_light.x, -to_light.y, in.height);
        // Not normalize(): a light at height 0 right on a pixel's centre
        // would be a zero vector, which some GPUs (Vulkan) turn into NaN, and
        // the bloom blur spreads a NaN over much of the frame.
        let l = v / max(length(v), 1e-6);
        shade = max(dot(n, l), 0.0) * m.b;
    }
    return vec4(in.color.rgb * f * shade, 0.0);
}
"#;

/// The sprites again, into the material target: each solid pixel of a sprite
/// writes its material (or nothing, for a sprite without maps), so what is
/// frontmost at a pixel decides how lights shade it.
pub const SPRITE_MATERIAL: &str = r#"
const VIEW: vec2<f32> = vec2<f32>(640.0, 360.0);

@group(0) @binding(0) var sheet: texture_2d<f32>;
@group(0) @binding(1) var pixel_sampler: sampler;
@group(1) @binding(0) var material: texture_2d<f32>;
@group(1) @binding(1) var material_sampler: sampler;

struct Sprite {
    @location(0) pos: vec2<f32>,
    @location(1) size: vec2<f32>,
    @location(2) uv_min: vec2<f32>,
    @location(3) uv_max: vec2<f32>,
    @location(4) color: vec4<f32>,
    @location(5) crimson: vec2<f32>,
};

struct VsOut {
    @builtin(position) clip: vec4<f32>,
    @location(0) uv: vec2<f32>,
    @location(1) alpha: f32,
};

@vertex
fn vs_main(@builtin(vertex_index) i: u32, s: Sprite) -> VsOut {
    var corners = array<vec2<f32>, 6>(
        vec2(0.0, 0.0), vec2(1.0, 0.0), vec2(0.0, 1.0),
        vec2(0.0, 1.0), vec2(1.0, 0.0), vec2(1.0, 1.0),
    );
    let c = corners[i];
    let ndc = (s.pos + c * s.size) / VIEW * 2.0 - 1.0;
    var out: VsOut;
    out.clip = vec4(ndc.x, -ndc.y, 0.0, 1.0);
    out.uv = mix(s.uv_min, s.uv_max, c);
    out.alpha = s.color.a;
    return out;
}

@fragment
fn fs_main(in: VsOut) -> @location(0) vec4<f32> {
    let coverage = textureSampleLevel(sheet, pixel_sampler, in.uv, 0.0).a * in.alpha;
    if coverage < 0.5 {
        discard;
    }
    return textureSampleLevel(material, material_sampler, in.uv, 0.0);
}
"#;

/// One oversized triangle covering the target, with uv 0..1 across it.
pub const FULLSCREEN: &str = r#"
struct VsOut {
    @builtin(position) clip: vec4<f32>,
    @location(0) uv: vec2<f32>,
};

@vertex
fn vs_main(@builtin(vertex_index) i: u32) -> VsOut {
    let uv = vec2(f32((i << 1u) & 2u), f32(i & 2u));
    var out: VsOut;
    out.clip = vec4(uv.x * 2.0 - 1.0, 1.0 - uv.y * 2.0, 0.0, 1.0);
    out.uv = uv;
    return out;
}
"#;

/// Must match `PostUniform` in post.rs.
pub const POST_UNIFORM: &str = r#"
struct Post {
    shaft: vec4<f32>,
    shaft_color: vec4<f32>,
    bloom: vec4<f32>,
    grade: vec4<f32>,
    lift: vec4<f32>,
    gain: vec4<f32>,
    time: vec4<f32>,
};
"#;

/// The lit world, keeping only what's brighter than the threshold.
pub const PREFILTER: &str = r#"
@group(0) @binding(0) var world: texture_2d<f32>;
@group(0) @binding(1) var light: texture_2d<f32>;
@group(0) @binding(2) var lin: sampler;
@group(0) @binding(3) var<uniform> post: Post;

@fragment
fn fs_main(in: VsOut) -> @location(0) vec4<f32> {
    let c = textureSample(world, lin, in.uv).rgb * textureSample(light, lin, in.uv).rgb;
    let threshold = post.bloom.y;
    let knee = post.bloom.z;
    let bright = max(c.r, max(c.g, c.b));
    // Soft knee: ease in over the knee instead of a hard cut.
    var soft = clamp(bright - threshold + knee, 0.0, 2.0 * knee);
    soft = soft * soft / (4.0 * knee + 1e-4);
    let w = max(soft, bright - threshold) / max(bright, 1e-4);
    return vec4(c * w, 1.0);
}
"#;

/// Dual-filter blur, down half a size.
pub const DOWN: &str = r#"
@group(0) @binding(0) var src: texture_2d<f32>;
@group(0) @binding(1) var lin: sampler;

@fragment
fn fs_main(in: VsOut) -> @location(0) vec4<f32> {
    let t = 1.0 / vec2<f32>(textureDimensions(src));
    var c = textureSample(src, lin, in.uv).rgb * 4.0;
    c += textureSample(src, lin, in.uv - t).rgb;
    c += textureSample(src, lin, in.uv + t).rgb;
    c += textureSample(src, lin, in.uv + vec2(t.x, -t.y)).rgb;
    c += textureSample(src, lin, in.uv - vec2(t.x, -t.y)).rgb;
    return vec4(c / 8.0, 1.0);
}
"#;

/// Dual-filter blur, up a size, added onto the level above.
pub const UP: &str = r#"
@group(0) @binding(0) var src: texture_2d<f32>;
@group(0) @binding(1) var lin: sampler;

@fragment
fn fs_main(in: VsOut) -> @location(0) vec4<f32> {
    let t = 1.0 / vec2<f32>(textureDimensions(src));
    var c = textureSample(src, lin, in.uv + vec2(-2.0 * t.x, 0.0)).rgb;
    c += textureSample(src, lin, in.uv + vec2(2.0 * t.x, 0.0)).rgb;
    c += textureSample(src, lin, in.uv + vec2(0.0, -2.0 * t.y)).rgb;
    c += textureSample(src, lin, in.uv + vec2(0.0, 2.0 * t.y)).rgb;
    c += textureSample(src, lin, in.uv + vec2(-t.x, -t.y)).rgb * 2.0;
    c += textureSample(src, lin, in.uv + vec2(t.x, -t.y)).rgb * 2.0;
    c += textureSample(src, lin, in.uv + vec2(-t.x, t.y)).rgb * 2.0;
    c += textureSample(src, lin, in.uv + vec2(t.x, t.y)).rgb * 2.0;
    return vec4(c / 12.0, 1.0);
}
"#;

/// God rays: march from each pixel towards the light, gathering the bright
/// pass near it, each step fainter.
pub const SHAFTS: &str = r#"
@group(0) @binding(0) var bright: texture_2d<f32>;
@group(0) @binding(1) var lin: sampler;
@group(0) @binding(2) var<uniform> post: Post;

const STEPS: i32 = 48;
const ASPECT: f32 = 360.0 / 640.0;

@fragment
fn fs_main(in: VsOut) -> @location(0) vec4<f32> {
    let centre = post.shaft.xy;
    let decay = post.shaft.w;
    let reach = post.shaft_color.w;
    let step = (in.uv - centre) / f32(STEPS) * 0.9;
    var uv = in.uv;
    var fall = 1.0;
    var sum = vec3(0.0);
    for (var i = 0; i < STEPS; i += 1) {
        uv -= step;
        let d = (uv - centre) * vec2(1.0, ASPECT);
        // Only the glow around the light casts rays.
        let near = 1.0 - smoothstep(0.0, reach, length(d));
        sum += textureSampleLevel(bright, lin, uv, 0.0).rgb * near * fall;
        fall *= decay;
    }
    return vec4(sum / f32(STEPS) * post.shaft_color.rgb, 1.0);
}
"#;

/// Lights the world, adds bloom and rays, tone maps and grades.
pub const COMPOSITE: &str = r#"
@group(0) @binding(0) var world: texture_2d<f32>;
@group(0) @binding(1) var light: texture_2d<f32>;
@group(0) @binding(2) var bloom: texture_2d<f32>;
@group(0) @binding(3) var shafts: texture_2d<f32>;
@group(0) @binding(4) var lin: sampler;
@group(0) @binding(5) var pixel: sampler;
@group(0) @binding(6) var<uniform> post: Post;

// Leaves colours below the knee as drawn and rolls highlights off softly.
fn shoulder(x: vec3<f32>) -> vec3<f32> {
    let k = 0.8;
    let over = max(x - k, vec3(0.0));
    let rolled = k + (1.0 - k) * (1.0 - exp(-over / (1.0 - k)));
    return select(x, rolled, x > vec3(k));
}

fn hash(p: vec2<f32>) -> f32 {
    return fract(sin(dot(p, vec2(12.9898, 78.233))) * 43758.5453);
}

@fragment
fn fs_main(in: VsOut) -> @location(0) vec4<f32> {
    var c = textureSample(world, pixel, in.uv).rgb * textureSample(light, lin, in.uv).rgb;
    c += textureSample(bloom, lin, in.uv).rgb * post.bloom.x;
    c += textureSample(shafts, lin, in.uv).rgb * post.shaft.z;
    c = shoulder(c * post.bloom.w);

    // Grade: lift the shadows, tint the highlights, then saturation and
    // contrast around mid grey.
    c = c * post.gain.rgb + post.lift.rgb * (1.0 - c);
    let luma = dot(c, vec3(0.2126, 0.7152, 0.0722));
    c = mix(vec3(luma), c, post.grade.x);
    c = max((c - 0.18) * post.grade.y + 0.18, vec3(0.0));

    // Vignette: darken towards the corners to frame the action.
    let d = (in.uv - vec2(0.5)) * vec2(1.0, 0.8);
    c *= 1.0 - smoothstep(0.3, 0.75, length(d)) * post.grade.z;

    let n = hash(in.uv * 1000.0 + post.time.x) - 0.5;
    c += n * post.grade.w;
    return vec4(c, 1.0);
}
"#;

/// A fog volume: drifting value-noise wisps, feathered at the edges, either
/// dithered per world pixel or smooth, and bent by the frame's stirs.
pub const FOG: &str = r#"
const VIEW: vec2<f32> = vec2<f32>(640.0, 360.0);
const MAX_STIRS: u32 = 24u;

struct Stirs {
    count: vec4<u32>,
    at: array<vec4<f32>, 24>,
    shove: array<vec4<f32>, 24>,
};

@group(0) @binding(0) var<uniform> stirs: Stirs;

struct Fog {
    @location(0) pos: vec2<f32>,
    @location(1) size: vec2<f32>,
    @location(2) color: vec4<f32>,
    @location(3) shape: vec4<f32>,
    @location(4) motion: vec4<f32>,
    @location(5) style: vec4<f32>,
    @location(6) extent: vec4<f32>,
};

struct VsOut {
    @builtin(position) clip: vec4<f32>,
    @location(0) local: vec2<f32>,
    @location(1) color: vec4<f32>,
    @location(2) shape: vec4<f32>,
    @location(3) motion: vec4<f32>,
    @location(4) size: vec2<f32>,
    @location(5) style: vec4<f32>,
    @location(6) origin: vec2<f32>,
    @location(7) extent: vec4<f32>,
};

@vertex
fn vs_main(@builtin(vertex_index) i: u32, f: Fog) -> VsOut {
    var corners = array<vec2<f32>, 6>(
        vec2(0.0, 0.0), vec2(1.0, 0.0), vec2(0.0, 1.0),
        vec2(0.0, 1.0), vec2(1.0, 0.0), vec2(1.0, 1.0),
    );
    let c = corners[i];
    // Slanted: the whole volume is sheared down by `slope` per pixel right.
    let sheared = c * f.size + vec2(0.0, c.x * f.size.x * f.style.z);
    let ndc = (f.pos + sheared) / VIEW * 2.0 - 1.0;
    var out: VsOut;
    out.clip = vec4(ndc.x, -ndc.y, 0.0, 1.0);
    out.local = c * f.size;
    out.color = f.color;
    out.shape = f.shape;
    out.motion = f.motion;
    out.size = f.size;
    out.style = f.style;
    out.origin = f.pos;
    out.extent = f.extent;
    return out;
}

fn hash(p: vec2<f32>) -> f32 {
    let q = fract(p * vec2(123.34, 456.21));
    let r = q + dot(q, q + 45.32);
    return fract(r.x * r.y);
}

fn noise(p: vec2<f32>) -> f32 {
    let i = floor(p);
    let f = fract(p);
    let u = f * f * (3.0 - 2.0 * f);
    let a = hash(i);
    let b = hash(i + vec2(1.0, 0.0));
    let c = hash(i + vec2(0.0, 1.0));
    let d = hash(i + vec2(1.0, 1.0));
    return mix(mix(a, b, u.x), mix(c, d, u.x), u.y);
}

fn fbm(p: vec2<f32>, octaves: i32) -> f32 {
    var sum = 0.0;
    var amp = 0.5;
    var q = p;
    for (var i = 0; i < octaves; i += 1) {
        sum += noise(q) * amp;
        q = q * 2.03 + vec2(17.1, 9.7);
        amp *= 0.5;
    }
    return sum;
}

// 4x4 ordered dither, 0..1.
fn bayer(p: vec2<f32>) -> f32 {
    let x = u32(p.x) % 4u;
    let y = u32(p.y) % 4u;
    var m = array<f32, 16>(
        0.0, 8.0, 2.0, 10.0,
        12.0, 4.0, 14.0, 6.0,
        3.0, 11.0, 1.0, 9.0,
        15.0, 7.0, 13.0, 5.0,
    );
    return (m[y * 4u + x] + 0.5) / 16.0;
}

@fragment
fn fs_main(in: VsOut) -> @location(0) vec4<f32> {
    let volumetric = in.style.x > 0.5;
    let response = in.style.y;
    // Pixel fog snaps to the world pixel so it shares the art's grid;
    // volumetric fog keeps every screen pixel.
    var px = in.local;
    if !volumetric {
        px = floor(in.local) + 0.5;
    }
    let world = in.origin + px + vec2(0.0, px.x * in.style.z);

    // Stirs bend the field the noise is read from, and thin it out.
    var bend = vec2(0.0);
    var parted = 0.0;
    if response > 0.0 {
        for (var i = 0u; i < min(stirs.count.x, MAX_STIRS); i += 1u) {
            let at = stirs.at[i];
            let shove = stirs.shove[i];
            let d = world - at.xy;
            let dist = length(d);
            let k = exp(-(dist * dist) / (at.z * at.z));
            let dir = d / max(dist, 0.001);
            let around = vec2(-dir.y, dir.x);
            // Read the noise from where the fog came from: pushed out from
            // the centre, along the shove, and round the swirl.
            bend += (dir * at.w * at.z * 0.6 + shove.xy + around * shove.z) * k;
            parted += at.w * k;
        }
        bend *= response;
        parted = clamp(parted * response, 0.0, 0.95);
    }

    let density = in.shape.x;
    let scale = in.shape.y;
    let drift = in.shape.zw;
    let time = in.motion.x;
    let seed = in.motion.y;
    let p = (px + in.extent.zw - bend) / scale + vec2(seed * 7.3, seed * 3.1);
    let moved = drift * time / scale;
    let octaves = select(4, 5, volumetric);
    // Two layers sliding at different speeds churn the wisps as they go.
    let warp = fbm(p * 0.6 - moved * 0.35 + vec2(time * 0.03, 0.0), octaves);
    let n = fbm(p - moved + vec2(warp * 1.6, warp * 0.6), octaves);
    var wisps = smoothstep(0.36, 0.74, n);
    if volumetric {
        // Softer edges than the pixel style's, so it reads as air.
        wisps = smoothstep(0.26, 0.8, n);
    }

    // Feather the top and bottom by their share of the height, and the sides.
    let v = px.y / in.size.y;
    let top = select(1.0, smoothstep(0.0, in.motion.z, v), in.motion.z > 0.0);
    let bottom = select(1.0, smoothstep(0.0, in.motion.w, 1.0 - v), in.motion.w > 0.0);
    let h = px.x / in.size.x;
    let left = select(1.0, smoothstep(0.0, in.extent.x, h), in.extent.x > 0.0);
    let right = select(1.0, smoothstep(0.0, in.extent.y, 1.0 - h), in.extent.y > 0.0);
    let sides = left * right;
    let a = density * wisps * top * bottom * sides * (1.0 - parted);

    if volumetric {
        if a <= 0.002 {
            discard;
        }
        // Thick cores shadow themselves; thin edges catch more light.
        let shade = mix(1.2, 0.78, wisps);
        return vec4(in.color.rgb * shade, a * in.color.a);
    }
    // Quantise to a few steps of opacity, dithered between them.
    let steps = 6.0;
    let level = floor(a * steps + bayer(px)) / steps;
    if level <= 0.0 {
        discard;
    }
    return vec4(in.color.rgb, level * in.color.a);
}
"#;

/// A body of water: the world behind it bent by ripples and tinted with
/// depth, the world above it mirrored near the surface, drifting caustics
/// and a bright line along the surface. Reads the frame drawn so far by
/// texel, so it must be drawn from a copy of the target.
pub const WATER: &str = r#"
const VIEW: vec2<f32> = vec2<f32>(640.0, 360.0);

@group(0) @binding(0) var grab: texture_2d<f32>;
@group(0) @binding(1) var heights: texture_2d<f32>;

struct Water {
    @location(0) pos: vec2<f32>,
    @location(1) size: vec2<f32>,
    @location(2) info: vec4<f32>,
    @location(3) shallow: vec4<f32>,
    @location(4) deep: vec4<f32>,
    @location(5) highlight: vec4<f32>,
    @location(6) optics: vec4<f32>,
    @location(7) motion: vec4<f32>,
};

struct VsOut {
    @builtin(position) clip: vec4<f32>,
    // From the resting surface's left end, in view units; y down.
    @location(0) local: vec2<f32>,
    @location(1) @interpolate(flat) origin: vec2<f32>,
    @location(2) @interpolate(flat) size: vec2<f32>,
    @location(3) @interpolate(flat) info: vec4<f32>,
    @location(4) @interpolate(flat) shallow: vec4<f32>,
    @location(5) @interpolate(flat) deep: vec4<f32>,
    @location(6) @interpolate(flat) highlight: vec4<f32>,
    @location(7) @interpolate(flat) optics: vec4<f32>,
    @location(8) @interpolate(flat) motion: vec4<f32>,
};

@vertex
fn vs_main(@builtin(vertex_index) i: u32, w: Water) -> VsOut {
    var corners = array<vec2<f32>, 6>(
        vec2(0.0, 0.0), vec2(1.0, 0.0), vec2(0.0, 1.0),
        vec2(0.0, 1.0), vec2(1.0, 0.0), vec2(1.0, 1.0),
    );
    let c = corners[i];
    let margin = w.info.w;
    let local = vec2(c.x * w.size.x, -margin + c.y * (w.size.y + margin));
    let ndc = (w.pos + local) / VIEW * 2.0 - 1.0;
    var out: VsOut;
    out.clip = vec4(ndc.x, -ndc.y, 0.0, 1.0);
    out.local = local;
    out.origin = w.pos;
    out.size = w.size;
    out.info = w.info;
    out.shallow = w.shallow;
    out.deep = w.deep;
    out.highlight = w.highlight;
    out.optics = w.optics;
    out.motion = w.motion;
    return out;
}

// The simulated surface at `x` view units along a body `w` wide.
fn height(row: i32, count: i32, x: f32, w: f32) -> f32 {
    if count <= 1 {
        return textureLoad(heights, vec2<i32>(0, row), 0).r;
    }
    let f = clamp(x / max(w, 0.001), 0.0, 1.0) * f32(count - 1);
    let i0 = i32(floor(f));
    let i1 = min(i0 + 1, count - 1);
    let a = textureLoad(heights, vec2<i32>(i0, row), 0).r;
    let b = textureLoad(heights, vec2<i32>(i1, row), 0).r;
    return mix(a, b, fract(f));
}

// The ambient swell at world pixel `x` along the body, -1..1.
fn swell(x: f32, t: f32) -> f32 {
    return sin(x * 0.045 + t * 1.3) * 0.55 + sin(x * 0.11 - t * 2.1) * 0.3
        + sin(x * 0.23 + t * 3.3) * 0.15;
}

// The world drawn so far at a point in view units, clamped to the frame.
fn fetch(p: vec2<f32>) -> vec3<f32> {
    let dims = vec2<f32>(textureDimensions(grab));
    let t = clamp(p * dims / VIEW, vec2(0.0), dims - 1.0);
    return textureLoad(grab, vec2<i32>(t), 0).rgb;
}

fn hash(p: vec2<f32>) -> f32 {
    let q = fract(p * vec2(123.34, 456.21));
    let r = q + dot(q, q + 45.32);
    return fract(r.x * r.y);
}

fn noise(p: vec2<f32>) -> f32 {
    let i = floor(p);
    let f = fract(p);
    let u = f * f * (3.0 - 2.0 * f);
    let a = hash(i);
    let b = hash(i + vec2(1.0, 0.0));
    let c = hash(i + vec2(0.0, 1.0));
    let d = hash(i + vec2(1.0, 1.0));
    return mix(mix(a, b, u.x), mix(c, d, u.x), u.y);
}

// 4x4 ordered dither, 0..1.
fn bayer(p: vec2<f32>) -> f32 {
    let x = u32(abs(p.x)) % 4u;
    let y = u32(abs(p.y)) % 4u;
    var m = array<f32, 16>(
        0.0, 8.0, 2.0, 10.0,
        12.0, 4.0, 14.0, 6.0,
        3.0, 11.0, 1.0, 9.0,
        15.0, 7.0, 13.0, 5.0,
    );
    return (m[y * 4u + x] + 0.5) / 16.0;
}

// `v` (0..1) in `steps` dithered levels when pixel snapping.
fn levels(v: f32, steps: f32, cell: vec2<f32>, pixel: bool) -> f32 {
    if !pixel {
        return v;
    }
    return clamp(floor(v * steps + bayer(cell)) / steps, 0.0, 1.0);
}

@fragment
fn fs_main(in: VsOut) -> @location(0) vec4<f32> {
    let s = in.info.z;
    let row = i32(in.info.x);
    let count = i32(in.info.y);
    let pixel = in.motion.w > 0.5;
    let t = in.motion.z;
    var p = in.local;
    if pixel {
        p = (floor(p / s) + 0.5) * s;
    }
    let cell = floor(p / s);
    // World pixels along the body, and the surface over this column.
    let wx = p.x / s;
    let surf = height(row, count, p.x, in.size.x) + swell(wx, t) * in.motion.y * s;
    if p.y < surf {
        discard;
    }
    // World pixels under the surface.
    let d = (p.y - surf) / s;
    let screen = in.origin + p;

    // What's behind the water, bent sideways by the ripples.
    let wob = sin(wx * 0.19 + d * 0.42 - t * 2.6) * 0.65 + sin(wx * 0.071 + t * 1.9 + d * 0.13) * 0.35;
    let bend = in.optics.w * smoothstep(0.0, 4.0, d) * wob;
    let behind = fetch(screen + vec2(bend * s, 0.0));
    var col = behind * mix(vec3(1.0), in.shallow.rgb, in.shallow.a);
    let deepness = levels(clamp(d / in.optics.x, 0.0, 1.0), 6.0, cell, pixel);
    col = mix(col, in.deep.rgb, in.deep.a * deepness);

    // Caustics: bright ridges where two drifting noise fields cross.
    // Strongest just under the surface, where the light comes in.
    let cp = vec2(wx * 0.05, d * 0.08);
    let c1 = noise(cp + vec2(t * 0.3, t * 0.17));
    let c2 = noise(cp * 1.6 + vec2(7.3 - t * 0.23, 3.1 + t * 0.27));
    let ridge = pow(1.0 - abs(c1 - c2), 24.0);
    let light_in = smoothstep(1.0, 4.0, d) * exp(-d / (in.optics.x * 0.3));
    let caustic = levels(ridge * light_in, 3.0, cell, pixel);
    col += in.highlight.rgb * caustic * in.motion.x * 0.3;

    // The world above, mirrored about the moving surface and rippled.
    let rip = sin(d * 0.9 - t * 3.4 + wx * 0.06) * min(d * 0.09, 2.0) + wob * in.optics.w * 0.5;
    let mirror = vec2(screen.x + rip * s, in.origin.y + 2.0 * surf - p.y);
    var shine = in.optics.y * (1.0 - smoothstep(0.0, in.optics.z, d));
    // Nothing to mirror past the top of the frame.
    shine *= smoothstep(0.0, 6.0 * s, mirror.y);
    shine = levels(shine, 5.0, cell, pixel);
    let above = fetch(mirror) * mix(vec3(1.0), in.shallow.rgb, in.shallow.a * 0.5);
    col = mix(col, above, shine);

    // A bright lip along the surface, and glints where it's steep.
    let slope = (height(row, count, p.x + s, in.size.x) - height(row, count, p.x - s, in.size.x)) / (2.0 * s);
    var lip = 1.0 - step(1.0, d);
    if !pixel {
        lip = 1.0 - smoothstep(0.6, 1.6, d);
    }
    let glint = (1.0 - step(2.0, d)) * clamp(abs(slope) * 1.5, 0.0, 1.0);
    col = mix(col, in.highlight.rgb, in.highlight.a * max(lip, glint * 0.6));
    // Sparse twinkles just under the surface.
    let twinkle = hash(vec2(cell.x, floor(d)) + floor(t * 5.0));
    if twinkle > 0.996 && d < 7.0 && d >= 1.0 {
        col += in.highlight.rgb * 0.5;
    }
    return vec4(col, 1.0);
}
"#;
