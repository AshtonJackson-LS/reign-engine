//! WGSL sources. Kept inline until the asset pipeline can hot reload them.

pub const RECT: &str = r#"
// World view in world pixels; must match VIEW_WIDTH/VIEW_HEIGHT.
const VIEW: vec2<f32> = vec2<f32>(640.0, 360.0);

struct Rect {
    @location(0) pos: vec2<f32>,
    @location(1) size: vec2<f32>,
    @location(2) color: vec4<f32>,
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
    let px = rect.pos + corners[i] * rect.size;
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
pub const LIGHT: &str = r#"
const VIEW: vec2<f32> = vec2<f32>(640.0, 360.0);

struct Light {
    @location(0) pos: vec2<f32>,
    @location(1) radius: f32,
    @location(2) pad: f32,
    @location(3) color: vec4<f32>,
};

struct VsOut {
    @builtin(position) clip: vec4<f32>,
    @location(0) local: vec2<f32>,
    @location(1) color: vec4<f32>,
};

@vertex
fn vs_main(@builtin(vertex_index) i: u32, l: Light) -> VsOut {
    var corners = array<vec2<f32>, 6>(
        vec2(-1.0, -1.0), vec2(1.0, -1.0), vec2(-1.0, 1.0),
        vec2(-1.0, 1.0), vec2(1.0, -1.0), vec2(1.0, 1.0),
    );
    let c = corners[i];
    let ndc = (l.pos + c * l.radius) / VIEW * 2.0 - 1.0;
    var out: VsOut;
    out.clip = vec4(ndc.x, -ndc.y, 0.0, 1.0);
    out.local = c;
    out.color = l.color;
    return out;
}

@fragment
fn fs_main(in: VsOut) -> @location(0) vec4<f32> {
    let d2 = dot(in.local, in.local);
    // Smooth to zero at the radius, with a hot core like a real falloff.
    let edge = clamp(1.0 - d2, 0.0, 1.0);
    let f = edge * edge / (1.0 + 8.0 * d2);
    return vec4(in.color.rgb * f, 0.0);
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
};

@vertex
fn vs_main(@builtin(vertex_index) i: u32, f: Fog) -> VsOut {
    var corners = array<vec2<f32>, 6>(
        vec2(0.0, 0.0), vec2(1.0, 0.0), vec2(0.0, 1.0),
        vec2(0.0, 1.0), vec2(1.0, 0.0), vec2(1.0, 1.0),
    );
    let c = corners[i];
    let ndc = (f.pos + c * f.size) / VIEW * 2.0 - 1.0;
    var out: VsOut;
    out.clip = vec4(ndc.x, -ndc.y, 0.0, 1.0);
    out.local = c * f.size;
    out.color = f.color;
    out.shape = f.shape;
    out.motion = f.motion;
    out.size = f.size;
    out.style = f.style;
    out.origin = f.pos;
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
    let world = in.origin + px;

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
    let p = (px - bend) / scale + vec2(seed * 7.3, seed * 3.1);
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
    let sides = smoothstep(0.0, 0.08, h) * smoothstep(0.0, 0.08, 1.0 - h);
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
