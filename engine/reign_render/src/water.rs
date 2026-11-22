//! Water: reflective, refractive bodies of water drawn into the world.
//!
//! A body is a rectangle whose top edge is a moving surface: a row of
//! heights (from `reign_water`'s simulation, or anything else) plus a gentle
//! ambient swell the shader adds itself. Below the surface the shader shows
//! the world behind the water, bent by ripples and tinted towards the deep
//! colour with depth, with the world above the surface mirrored into it near
//! the top. A bright line traces the surface and shimmering caustic bands
//! drift through it.
//!
//! To see what's behind and above it, water reads the frame drawn so far: at
//! each run of water draws the world pass stops, the target is copied, and
//! the water is drawn from the copy. Everything queued before the water is
//! reflected and refracted by it; everything queued after is drawn over it.
//! Like fog, water lands in the HDR world before lighting, so lamps light it
//! and its bright surface blooms.

use crate::Color;

/// How a body of water looks. Distances are in world pixels.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct WaterStyle {
    /// Multiplies what's seen through the water just under the surface; its
    /// alpha is how strongly.
    pub shallow: Color,
    /// What the water turns to with depth; its alpha is the most it covers.
    pub deep: Color,
    /// How deep the water gets before it is fully `deep`.
    pub clarity: f32,
    /// How strongly the world above is mirrored at the surface, 0..1.
    pub reflection: f32,
    /// How far below the surface the reflection fades out.
    pub reflection_depth: f32,
    /// How far ripples bend what's seen through the water.
    pub refraction: f32,
    /// The line along the surface. Values above 1 (through `glow`) bloom.
    pub highlight: Color,
    /// Multiplies `highlight`'s colour, so the surface can glow in HDR.
    pub glow: f32,
    /// Strength of the light bands drifting through the water, 0..1.
    pub caustics: f32,
    /// Height of the ambient swell, on top of the simulated heights.
    pub swell: f32,
    /// Snap to the world pixel grid and dither between tints, to sit in
    /// with pixel art; off, the water is smooth at screen resolution.
    pub pixel: bool,
}

impl Default for WaterStyle {
    fn default() -> Self {
        WaterStyle {
            shallow: Color::rgba8(150, 215, 235, 150),
            deep: Color::rgba8(12, 34, 58, 235),
            clarity: 72.0,
            reflection: 0.6,
            reflection_depth: 96.0,
            refraction: 1.6,
            highlight: Color::rgba8(205, 240, 255, 230),
            glow: 1.6,
            caustics: 0.35,
            swell: 0.9,
            pixel: true,
        }
    }
}

/// One body of water to draw this frame, in world pixels.
#[derive(Clone, Copy, Debug)]
pub struct WaterSurface<'a> {
    /// The left end of the surface at rest.
    pub x: f32,
    /// The surface at rest; heights are measured from here, down positive.
    pub y: f32,
    pub w: f32,
    /// From the resting surface to the bottom of the body.
    pub depth: f32,
    /// The surface's displacement, evenly spaced from `x` to `x + w`. Empty
    /// for a flat surface (the swell still moves it). Past
    /// [`MAX_WATER_COLUMNS`] it is resampled.
    pub heights: &'a [f32],
    /// Seconds, for the swell, ripples and caustics.
    pub time: f32,
    pub style: WaterStyle,
}

/// The most bodies of water drawn in a frame; more are dropped.
pub const MAX_WATER_BODIES: usize = 32;
/// The most surface heights a body has on the GPU; more are resampled.
pub const MAX_WATER_COLUMNS: usize = 1024;

#[repr(C)]
#[derive(Clone, Copy, Debug, bytemuck::Pod, bytemuck::Zeroable)]
pub(crate) struct WaterInstance {
    /// The resting surface's left end, in view units.
    pos: [f32; 2],
    /// Width, and depth below the resting surface.
    size: [f32; 2],
    /// Heights row, column count, view units per world pixel, margin above
    /// the resting surface.
    info: [f32; 4],
    shallow: [f32; 4],
    deep: [f32; 4],
    highlight: [f32; 4],
    /// Clarity, reflection, reflection depth, refraction (world pixels).
    optics: [f32; 4],
    /// Caustics, swell (world pixels), time, pixel snapping (0 or 1).
    motion: [f32; 4],
}

/// Everything the renderer needs to draw water.
pub(crate) struct WaterPass {
    pipeline: wgpu::RenderPipeline,
    layout: wgpu::BindGroupLayout,
    buffer: wgpu::Buffer,
    capacity: usize,
    instances: Vec<WaterInstance>,
    /// Every body's heights, a row each, in view units.
    heights: Vec<f32>,
    heights_texture: wgpu::Texture,
    /// The copy of the world target the water reads, and its bind group;
    /// made at the target's size when first needed.
    grab: Option<Grab>,
}

struct Grab {
    texture: wgpu::Texture,
    bind_group: wgpu::BindGroup,
    size: (u32, u32),
}

const INITIAL_CAPACITY: usize = 4;

impl WaterPass {
    pub fn new(device: &wgpu::Device, format: wgpu::TextureFormat) -> Self {
        let layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("water layout"),
            entries: &[
                // The world drawn so far, read by texel.
                wgpu::BindGroupLayoutEntry {
                    binding: 0,
                    visibility: wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Texture {
                        sample_type: wgpu::TextureSampleType::Float { filterable: false },
                        view_dimension: wgpu::TextureViewDimension::D2,
                        multisampled: false,
                    },
                    count: None,
                },
                // The surface heights, a row per body.
                wgpu::BindGroupLayoutEntry {
                    binding: 1,
                    visibility: wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Texture {
                        sample_type: wgpu::TextureSampleType::Float { filterable: false },
                        view_dimension: wgpu::TextureViewDimension::D2,
                        multisampled: false,
                    },
                    count: None,
                },
            ],
        });
        let heights_texture = device.create_texture(&wgpu::TextureDescriptor {
            label: Some("water heights"),
            size: wgpu::Extent3d {
                width: MAX_WATER_COLUMNS as u32,
                height: MAX_WATER_BODIES as u32,
                depth_or_array_layers: 1,
            },
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: wgpu::TextureFormat::R32Float,
            usage: wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_DST,
            view_formats: &[],
        });
        WaterPass {
            pipeline: create_pipeline(device, &layout, format),
            layout,
            buffer: crate::create_instance_buffer::<WaterInstance>(device, INITIAL_CAPACITY),
            capacity: INITIAL_CAPACITY,
            instances: Vec::with_capacity(INITIAL_CAPACITY),
            heights: Vec::new(),
            heights_texture,
            grab: None,
        }
    }

    /// Queues a body, already scaled by the draw scale `s` and shifted by
    /// `offset`; returns how many are queued, or `None` if it was dropped.
    pub fn push(&mut self, water: &WaterSurface, offset: [f32; 2], s: f32) -> Option<u32> {
        if self.instances.len() >= MAX_WATER_BODIES {
            log::warn!("more than {MAX_WATER_BODIES} bodies of water in a frame: dropped");
            return None;
        }
        if water.w <= 0.0 || water.depth <= 0.0 {
            return None;
        }
        let row = self.instances.len();
        let columns = resample(water.heights, MAX_WATER_COLUMNS);
        let count = columns.len().max(1);
        // The quad reaches above the resting surface far enough for the
        // highest crest and the swell.
        let crest = columns.iter().fold(0.0f32, |m, &h| m.max(-h));
        let margin = (crest + water.style.swell.abs() + 2.0) * s;
        self.heights.resize((row + 1) * MAX_WATER_COLUMNS, 0.0);
        let start = row * MAX_WATER_COLUMNS;
        for (slot, h) in self.heights[start..start + count].iter_mut().zip(columns.iter()) {
            *slot = h * s;
        }
        if columns.is_empty() {
            self.heights[start] = 0.0;
        }
        let st = water.style;
        let mut highlight = st.highlight.to_linear();
        for c in &mut highlight[..3] {
            *c *= st.glow.max(0.0);
        }
        self.instances.push(WaterInstance {
            pos: [(water.x + offset[0]) * s, (water.y + offset[1]) * s],
            size: [water.w * s, water.depth * s],
            info: [row as f32, count as f32, s, margin],
            shallow: st.shallow.to_linear(),
            deep: st.deep.to_linear(),
            highlight,
            optics: [
                st.clarity.max(0.01),
                st.reflection.clamp(0.0, 1.0),
                st.reflection_depth.max(0.01),
                st.refraction,
            ],
            motion: [
                st.caustics.clamp(0.0, 1.0),
                st.swell,
                water.time,
                if st.pixel { 1.0 } else { 0.0 },
            ],
        });
        Some(self.instances.len() as u32)
    }

    pub fn upload(&mut self, device: &wgpu::Device, queue: &wgpu::Queue) {
        if self.instances.is_empty() {
            return;
        }
        if self.instances.len() > self.capacity {
            self.capacity = self.instances.len().next_power_of_two();
            self.buffer = crate::create_instance_buffer::<WaterInstance>(device, self.capacity);
        }
        queue.write_buffer(&self.buffer, 0, bytemuck::cast_slice(&self.instances));
        let rows = self.instances.len() as u32;
        queue.write_texture(
            wgpu::TexelCopyTextureInfo {
                texture: &self.heights_texture,
                mip_level: 0,
                origin: wgpu::Origin3d::ZERO,
                aspect: wgpu::TextureAspect::All,
            },
            bytemuck::cast_slice(&self.heights),
            wgpu::TexelCopyBufferLayout {
                offset: 0,
                bytes_per_row: Some(MAX_WATER_COLUMNS as u32 * 4),
                rows_per_image: Some(rows),
            },
            wgpu::Extent3d {
                width: MAX_WATER_COLUMNS as u32,
                height: rows,
                depth_or_array_layers: 1,
            },
        );
    }

    /// Copies the world drawn so far into the texture the water reads.
    pub fn grab(
        &mut self,
        device: &wgpu::Device,
        encoder: &mut wgpu::CommandEncoder,
        world: &wgpu::Texture,
    ) {
        let size = (world.width(), world.height());
        if self.grab.as_ref().is_none_or(|g| g.size != size) {
            let texture = device.create_texture(&wgpu::TextureDescriptor {
                label: Some("water grab"),
                size: wgpu::Extent3d { width: size.0, height: size.1, depth_or_array_layers: 1 },
                mip_level_count: 1,
                sample_count: 1,
                dimension: wgpu::TextureDimension::D2,
                format: world.format(),
                usage: wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_DST,
                view_formats: &[],
            });
            let grab_view = texture.create_view(&wgpu::TextureViewDescriptor::default());
            let heights_view =
                self.heights_texture.create_view(&wgpu::TextureViewDescriptor::default());
            let bind_group = device.create_bind_group(&wgpu::BindGroupDescriptor {
                label: Some("water bind group"),
                layout: &self.layout,
                entries: &[
                    wgpu::BindGroupEntry {
                        binding: 0,
                        resource: wgpu::BindingResource::TextureView(&grab_view),
                    },
                    wgpu::BindGroupEntry {
                        binding: 1,
                        resource: wgpu::BindingResource::TextureView(&heights_view),
                    },
                ],
            });
            self.grab = Some(Grab { texture, bind_group, size });
        }
        let grab = self.grab.as_ref().expect("made above");
        encoder.copy_texture_to_texture(
            world.as_image_copy(),
            grab.texture.as_image_copy(),
            wgpu::Extent3d { width: size.0, height: size.1, depth_or_array_layers: 1 },
        );
    }

    /// Draws bodies `start..end`, reading the last [`WaterPass::grab`].
    pub fn draw(&self, pass: &mut wgpu::RenderPass, start: u32, end: u32) {
        let Some(grab) = &self.grab else { return };
        pass.set_pipeline(&self.pipeline);
        pass.set_bind_group(0, &grab.bind_group, &[]);
        pass.set_vertex_buffer(0, self.buffer.slice(..));
        pass.draw(0..6, start..end);
    }

    pub fn clear(&mut self) {
        self.instances.clear();
        self.heights.clear();
    }
}

/// `heights` at most `max` long: as they are if they fit, else sampled
/// evenly (linearly) along their length.
fn resample(heights: &[f32], max: usize) -> std::borrow::Cow<'_, [f32]> {
    if heights.len() <= max {
        return std::borrow::Cow::Borrowed(heights);
    }
    let last = (heights.len() - 1) as f32;
    let out = (0..max)
        .map(|i| {
            let f = i as f32 / (max - 1) as f32 * last;
            let (i0, t) = (f.floor() as usize, f.fract());
            let i1 = (i0 + 1).min(heights.len() - 1);
            heights[i0] + (heights[i1] - heights[i0]) * t
        })
        .collect();
    std::borrow::Cow::Owned(out)
}

pub(crate) fn create_pipeline(
    device: &wgpu::Device,
    layout: &wgpu::BindGroupLayout,
    format: wgpu::TextureFormat,
) -> wgpu::RenderPipeline {
    let shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
        label: Some("water shader"),
        source: wgpu::ShaderSource::Wgsl(crate::shaders::WATER.into()),
    });
    let pipeline_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
        label: Some("water pipeline layout"),
        bind_group_layouts: &[Some(layout)],
        ..Default::default()
    });
    const ATTRS: [wgpu::VertexAttribute; 8] = wgpu::vertex_attr_array![
        0 => Float32x2, 1 => Float32x2, 2 => Float32x4, 3 => Float32x4, 4 => Float32x4,
        5 => Float32x4, 6 => Float32x4, 7 => Float32x4
    ];
    device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
        label: Some("water pipeline"),
        layout: Some(&pipeline_layout),
        vertex: wgpu::VertexState {
            module: &shader,
            entry_point: Some("vs_main"),
            compilation_options: Default::default(),
            buffers: &[Some(wgpu::VertexBufferLayout {
                array_stride: std::mem::size_of::<WaterInstance>() as u64,
                step_mode: wgpu::VertexStepMode::Instance,
                attributes: &ATTRS,
            })],
        },
        fragment: Some(wgpu::FragmentState {
            module: &shader,
            entry_point: Some("fs_main"),
            compilation_options: Default::default(),
            targets: &[Some(wgpu::ColorTargetState {
                format,
                // The water already holds what's behind it, so it replaces.
                blend: None,
                write_mask: wgpu::ColorWrites::ALL,
            })],
        }),
        primitive: wgpu::PrimitiveState::default(),
        depth_stencil: None,
        multisample: wgpu::MultisampleState::default(),
        multiview_mask: None,
        cache: None,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn water_shader_validates() {
        pollster::block_on(async {
            let instance = wgpu::Instance::new(
                wgpu::InstanceDescriptor::new_without_display_handle_from_env(),
            );
            let Ok(adapter) =
                instance.request_adapter(&wgpu::RequestAdapterOptions::default()).await
            else {
                eprintln!("No GPU adapter available for water pipeline validation");
                return;
            };
            let (device, _) =
                adapter.request_device(&wgpu::DeviceDescriptor::default()).await.unwrap();
            let scope = device.push_error_scope(wgpu::ErrorFilter::Validation);
            let _ = WaterPass::new(&device, crate::post::HDR_FORMAT);
            assert!(scope.pop().await.is_none(), "water shader or instance layout is invalid");
        });
    }

    /// A red block stands on the shore of a pool on the left, nothing on the
    /// right: just under the surface the left half mirrors red and the right
    /// doesn't, and deep down both are the deep colour.
    #[cfg(feature = "hooks")]
    #[test]
    fn water_mirrors_what_stands_above_it() {
        use crate::{Color, Lighting, Renderer};
        let format = wgpu::TextureFormat::Rgba8UnormSrgb;
        let mut r = match pollster::block_on(Renderer::headless((640, 360), format)) {
            Ok(r) => r,
            Err(e) => {
                eprintln!("No GPU adapter available ({e}): skipping the water test");
                return;
            }
        };
        let scope = r.device().push_error_scope(wgpu::ErrorFilter::Validation);
        r.set_lighting(Lighting { vignette: 0.0, ..Lighting::default() });
        let grey = Color::rgb8(90, 90, 90);
        r.rect(0.0, 0.0, 640.0, 360.0, grey);
        r.rect(100.0, 120.0, 80.0, 80.0, Color::rgb8(255, 0, 0));
        let flat = [0.0; 64];
        let style =
            WaterStyle { swell: 0.0, refraction: 0.0, caustics: 0.0, ..WaterStyle::default() };
        r.water(&WaterSurface {
            x: 0.0,
            y: 200.0,
            w: 640.0,
            depth: 160.0,
            heights: &flat,
            time: 0.0,
            style,
        });
        r.present(Color::BLACK);
        assert!(pollster::block_on(scope.pop()).is_none(), "a validation error");
        let px = r.read_offscreen().unwrap();
        let at = |x: usize, y: usize| {
            let i = (y * 640 + x) * 4;
            [px[i], px[i + 1], px[i + 2]]
        };
        let (left, right) = (at(140, 206), at(500, 206));
        assert!(left[0] > left[1] + 40, "the red block is mirrored: {left:?}");
        assert!(right[0] <= right[1] + 10, "nothing red to mirror: {right:?}");
        let (deep_l, deep_r) = (at(140, 350), at(500, 350));
        assert_eq!(deep_l, deep_r, "deep down the water is all one colour");
        // The surface line is the brightest row.
        let lip = at(500, 200);
        assert!(lip[2] > right[2], "the surface glows: {lip:?} over {right:?}");
    }

    #[test]
    fn heights_that_fit_are_kept_and_long_rows_are_resampled_end_to_end() {
        let short = [1.0, 2.0, 3.0];
        assert_eq!(&*resample(&short, 8), &short);
        let long: Vec<f32> = (0..2001).map(|i| i as f32).collect();
        let r = resample(&long, 1001);
        assert_eq!(r.len(), 1001);
        assert_eq!(r[0], 0.0);
        assert_eq!(r[1000], 2000.0);
        assert!((r[500] - 1000.0).abs() < 1e-3);
    }
}
