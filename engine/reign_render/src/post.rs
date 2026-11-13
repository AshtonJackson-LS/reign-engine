//! Lighting and post effects, after Unity's URP 2D renderer.
//!
//! The world is drawn into an HDR target. Lights are drawn into a light map
//! at view size, cleared to the ambient colour, and the world is multiplied
//! by it. Whatever ends up brighter than the threshold blooms through a
//! chain of half-size targets (dual-filter blur). Light shafts are a radial
//! blur of the bright parts around one light, usually the moon. The final
//! pass adds it all up, tone maps, grades and vignettes it into the window.

use crate::{Color, VIEW_HEIGHT, VIEW_WIDTH, shaders};

pub(crate) const HDR_FORMAT: wgpu::TextureFormat = wgpu::TextureFormat::Rgba16Float;
const INITIAL_LIGHT_CAPACITY: usize = 64;
/// Most bloom levels; fewer when the target is small.
const BLOOM_LEVELS: usize = 6;

/// A 2D point light, in world pixels. Brightens everything within `radius`,
/// softly, most at the centre. `intensity` above 1 overexposes into bloom.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Light {
    pub x: f32,
    pub y: f32,
    pub radius: f32,
    pub color: Color,
    pub intensity: f32,
}

/// God rays streaming out from around one light.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Shafts {
    /// Centre in world pixels.
    pub x: f32,
    pub y: f32,
    pub color: Color,
    pub strength: f32,
    /// How much of the light carries per step out along a ray, under 1.
    pub decay: f32,
    /// Only bright pixels within this many world pixels cast rays.
    pub reach: f32,
}

/// How the frame is lit and graded. The default leaves the art exactly as
/// drawn: white ambient, no bloom, no grading, just the vignette.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Lighting {
    /// The light map before any light is added: what unlit areas get.
    pub ambient: Color,
    pub ambient_intensity: f32,
    pub exposure: f32,
    pub bloom: f32,
    /// Brightness where bloom starts, in linear light.
    pub bloom_threshold: f32,
    pub saturation: f32,
    /// Around mid grey.
    pub contrast: f32,
    /// Added into the shadows.
    pub lift: Color,
    /// Multiplies the highlights.
    pub gain: Color,
    pub vignette: f32,
    pub grain: f32,
    pub shafts: Option<Shafts>,
}

impl Default for Lighting {
    fn default() -> Self {
        Lighting {
            ambient: Color::WHITE,
            ambient_intensity: 1.0,
            exposure: 1.0,
            bloom: 0.0,
            bloom_threshold: 1.0,
            saturation: 1.0,
            contrast: 1.0,
            lift: Color::BLACK,
            gain: Color::WHITE,
            vignette: 0.55,
            grain: 0.0,
            shafts: None,
        }
    }
}

#[repr(C)]
#[derive(Clone, Copy, Debug, bytemuck::Pod, bytemuck::Zeroable)]
struct LightInstance {
    pos: [f32; 2],
    radius: f32,
    _pad: f32,
    color: [f32; 4],
}

/// Mirrors `Post` in the WGSL; every field a vec4 so the layouts agree.
#[repr(C)]
#[derive(Clone, Copy, Debug, bytemuck::Pod, bytemuck::Zeroable)]
struct PostUniform {
    /// Centre (uv), strength, decay.
    shaft: [f32; 4],
    /// Colour, reach (in uv, along x).
    shaft_color: [f32; 4],
    /// Strength, threshold, knee, exposure.
    bloom: [f32; 4],
    /// Saturation, contrast, vignette, grain.
    grade: [f32; 4],
    lift: [f32; 4],
    gain: [f32; 4],
    /// Time in seconds, then unused.
    time: [f32; 4],
}

struct Mip {
    view: wgpu::TextureView,
    /// Reads this level, for the next pass down or up.
    read: wgpu::BindGroup,
}

/// Targets sized to the window, rebuilt when the scale changes.
struct Targets {
    light: wgpu::TextureView,
    mips: Vec<Mip>,
    shafts: wgpu::TextureView,
    prefilter: wgpu::BindGroup,
    shafts_read: wgpu::BindGroup,
    composite: wgpu::BindGroup,
}

pub(crate) struct Post {
    light_pipeline: wgpu::RenderPipeline,
    prefilter_pipeline: wgpu::RenderPipeline,
    down_pipeline: wgpu::RenderPipeline,
    up_pipeline: wgpu::RenderPipeline,
    shafts_pipeline: wgpu::RenderPipeline,
    composite_pipeline: wgpu::RenderPipeline,
    tex_layout: wgpu::BindGroupLayout,
    prefilter_layout: wgpu::BindGroupLayout,
    shafts_layout: wgpu::BindGroupLayout,
    composite_layout: wgpu::BindGroupLayout,
    linear: wgpu::Sampler,
    uniform: wgpu::Buffer,
    light_buffer: wgpu::Buffer,
    light_capacity: usize,
    lights: Vec<LightInstance>,
    pub lighting: Lighting,
    targets: Targets,
    started: std::time::Instant,
}

impl Post {
    /// `tex_layout` is one texture and a filtering sampler; `world` the HDR
    /// world target, `size` its size in pixels.
    pub fn new(
        device: &wgpu::Device,
        tex_layout: &wgpu::BindGroupLayout,
        nearest: &wgpu::Sampler,
        surface_format: wgpu::TextureFormat,
        world: &wgpu::TextureView,
        size: (u32, u32),
    ) -> Self {
        let linear = device.create_sampler(&wgpu::SamplerDescriptor {
            label: Some("linear sampler"),
            mag_filter: wgpu::FilterMode::Linear,
            min_filter: wgpu::FilterMode::Linear,
            address_mode_u: wgpu::AddressMode::ClampToEdge,
            address_mode_v: wgpu::AddressMode::ClampToEdge,
            ..Default::default()
        });
        let uniform = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("post uniform"),
            size: std::mem::size_of::<PostUniform>() as u64,
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        let prefilter_layout = layout(device, "prefilter bindings", &[Tex, Tex, Samp, Uniform]);
        let shafts_layout = layout(device, "shafts bindings", &[Tex, Samp, Uniform]);
        let composite_layout =
            layout(device, "composite bindings", &[Tex, Tex, Tex, Tex, Samp, Samp, Uniform]);

        let light_pipeline = light_pipeline(device);
        let fullscreen = |label: &str,
                          fragment: &str,
                          layout: &wgpu::BindGroupLayout,
                          format: wgpu::TextureFormat,
                          blend: Option<wgpu::BlendState>| {
            fullscreen_pipeline(device, label, fragment, layout, format, blend)
        };
        let prefilter_pipeline =
            fullscreen("prefilter", shaders::PREFILTER, &prefilter_layout, HDR_FORMAT, None);
        let down_pipeline = fullscreen("bloom down", shaders::DOWN, tex_layout, HDR_FORMAT, None);
        let up_pipeline = fullscreen(
            "bloom up",
            shaders::UP,
            tex_layout,
            HDR_FORMAT,
            Some(wgpu::BlendState::ADDITIVE),
        );
        let shafts_pipeline =
            fullscreen("shafts", shaders::SHAFTS, &shafts_layout, HDR_FORMAT, None);
        let composite_pipeline =
            fullscreen("composite", shaders::COMPOSITE, &composite_layout, surface_format, None);
        let light_buffer =
            crate::create_instance_buffer::<LightInstance>(device, INITIAL_LIGHT_CAPACITY);

        let targets = Targets::new(
            device,
            tex_layout,
            &prefilter_layout,
            &shafts_layout,
            &composite_layout,
            &linear,
            nearest,
            &uniform,
            world,
            size,
        );
        Post {
            light_pipeline,
            prefilter_pipeline,
            down_pipeline,
            up_pipeline,
            shafts_pipeline,
            composite_pipeline,
            tex_layout: tex_layout.clone(),
            prefilter_layout,
            shafts_layout,
            composite_layout,
            linear,
            uniform,
            light_buffer,
            light_capacity: INITIAL_LIGHT_CAPACITY,
            lights: Vec::new(),
            lighting: Lighting::default(),
            targets,
            started: std::time::Instant::now(),
        }
    }

    /// Rebuilds the targets for a new world target.
    pub fn resize(
        &mut self,
        device: &wgpu::Device,
        nearest: &wgpu::Sampler,
        world: &wgpu::TextureView,
        size: (u32, u32),
    ) {
        self.targets = Targets::new(
            device,
            &self.tex_layout,
            &self.prefilter_layout,
            &self.shafts_layout,
            &self.composite_layout,
            &self.linear,
            nearest,
            &self.uniform,
            world,
            size,
        );
    }

    pub fn clear_lights(&mut self) {
        self.lights.clear();
    }

    pub fn push_light(&mut self, light: Light, offset: [f32; 2]) {
        let [r, g, b, _] = light.color.to_linear();
        let i = light.intensity;
        self.lights.push(LightInstance {
            pos: [light.x + offset[0], light.y + offset[1]],
            radius: light.radius,
            _pad: 0.0,
            color: [r * i, g * i, b * i, 1.0],
        });
    }

    /// Lights the world target and composites it into `frame`, inside the
    /// window's `viewport`.
    pub fn render(
        &mut self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        encoder: &mut wgpu::CommandEncoder,
        frame: &wgpu::TextureView,
        viewport: (u32, u32, u32, u32),
    ) {
        let l = self.lighting;
        let shafts = l.shafts.unwrap_or(Shafts {
            x: 0.0,
            y: 0.0,
            color: Color::BLACK,
            strength: 0.0,
            decay: 0.9,
            reach: 1.0,
        });
        let rgb = |c: Color| {
            let [r, g, b, _] = c.to_linear();
            [r, g, b, 0.0]
        };
        let [sr, sg, sb, _] = shafts.color.to_linear();
        let uniform = PostUniform {
            shaft: [
                shafts.x / VIEW_WIDTH as f32,
                shafts.y / VIEW_HEIGHT as f32,
                shafts.strength,
                shafts.decay,
            ],
            shaft_color: [sr, sg, sb, shafts.reach / VIEW_WIDTH as f32],
            bloom: [l.bloom, l.bloom_threshold, l.bloom_threshold * 0.5, l.exposure],
            grade: [l.saturation, l.contrast, l.vignette, l.grain],
            lift: rgb(l.lift),
            gain: rgb(l.gain),
            time: [self.started.elapsed().as_secs_f32() % 1000.0, 0.0, 0.0, 0.0],
        };
        queue.write_buffer(&self.uniform, 0, bytemuck::bytes_of(&uniform));
        if self.lights.len() > self.light_capacity {
            self.light_capacity = self.lights.len().next_power_of_two();
            self.light_buffer =
                crate::create_instance_buffer::<LightInstance>(device, self.light_capacity);
        }
        queue.write_buffer(&self.light_buffer, 0, bytemuck::cast_slice(&self.lights));

        let t = &self.targets;
        let [ar, ag, ab, _] = l.ambient.to_linear();
        let ambient = Color {
            r: ar * l.ambient_intensity,
            g: ag * l.ambient_intensity,
            b: ab * l.ambient_intensity,
            a: 1.0,
        };
        {
            let mut pass = crate::begin_pass(encoder, &t.light, Some(ambient), "light pass");
            if !self.lights.is_empty() {
                pass.set_pipeline(&self.light_pipeline);
                pass.set_vertex_buffer(0, self.light_buffer.slice(..));
                pass.draw(0..6, 0..self.lights.len() as u32);
            }
        }
        let fullscreen = |encoder: &mut wgpu::CommandEncoder,
                          target: &wgpu::TextureView,
                          clear: bool,
                          pipeline: &wgpu::RenderPipeline,
                          bind: &wgpu::BindGroup,
                          label: &str| {
            let load = clear.then_some(Color::BLACK);
            let mut pass = crate::begin_pass(encoder, target, load, label);
            pass.set_pipeline(pipeline);
            pass.set_bind_group(0, bind, &[]);
            pass.draw(0..3, 0..1);
        };
        fullscreen(
            encoder,
            &t.mips[0].view,
            true,
            &self.prefilter_pipeline,
            &t.prefilter,
            "prefilter",
        );
        for i in 1..t.mips.len() {
            let (src, dst) = (&t.mips[i - 1], &t.mips[i]);
            fullscreen(encoder, &dst.view, true, &self.down_pipeline, &src.read, "bloom down");
        }
        // Rays come from the bright pass before the upsample blurs it.
        fullscreen(encoder, &t.shafts, true, &self.shafts_pipeline, &t.shafts_read, "shafts");
        for i in (1..t.mips.len()).rev() {
            let (dst, src) = (&t.mips[i - 1], &t.mips[i]);
            fullscreen(encoder, &dst.view, false, &self.up_pipeline, &src.read, "bloom up");
        }
        {
            let mut pass = crate::begin_pass(encoder, frame, Some(Color::BLACK), "composite");
            let (x, y, w, h) = viewport;
            pass.set_viewport(x as f32, y as f32, w as f32, h as f32, 0.0, 1.0);
            pass.set_pipeline(&self.composite_pipeline);
            pass.set_bind_group(0, &t.composite, &[]);
            pass.draw(0..3, 0..1);
        }
        self.lights.clear();
    }
}

impl Targets {
    #[allow(clippy::too_many_arguments)]
    fn new(
        device: &wgpu::Device,
        tex_layout: &wgpu::BindGroupLayout,
        prefilter_layout: &wgpu::BindGroupLayout,
        shafts_layout: &wgpu::BindGroupLayout,
        composite_layout: &wgpu::BindGroupLayout,
        linear: &wgpu::Sampler,
        nearest: &wgpu::Sampler,
        uniform: &wgpu::Buffer,
        world: &wgpu::TextureView,
        size: (u32, u32),
    ) -> Self {
        let light = hdr_target(device, "light map", (VIEW_WIDTH, VIEW_HEIGHT));
        let mut mips = Vec::new();
        let (mut w, mut h) = (size.0 / 2, size.1 / 2);
        while mips.len() < BLOOM_LEVELS && w >= 8 && h >= 8 {
            let view = hdr_target(device, "bloom level", (w, h));
            let read = crate::bind_texture(device, tex_layout, &view, linear);
            mips.push(Mip { view, read });
            (w, h) = (w / 2, h / 2);
        }
        // Rays are soft; a quarter of the target is plenty.
        let ray_size = (size.0 / 4).max(1);
        let shafts = hdr_target(device, "shafts", (ray_size, (size.1 / 4).max(1)));
        let bright = &mips[1.min(mips.len() - 1)].view;
        let prefilter = bind(
            device,
            prefilter_layout,
            &[Res::Tex(world), Res::Tex(&light), Res::Samp(linear), Res::Buf(uniform)],
        );
        let shafts_read =
            bind(device, shafts_layout, &[Res::Tex(bright), Res::Samp(linear), Res::Buf(uniform)]);
        let composite = bind(
            device,
            composite_layout,
            &[
                Res::Tex(world),
                Res::Tex(&light),
                Res::Tex(&mips[0].view),
                Res::Tex(&shafts),
                Res::Samp(linear),
                Res::Samp(nearest),
                Res::Buf(uniform),
            ],
        );
        Targets { light, mips, shafts, prefilter, shafts_read, composite }
    }
}

pub(crate) fn hdr_target(
    device: &wgpu::Device,
    label: &str,
    size: (u32, u32),
) -> wgpu::TextureView {
    device
        .create_texture(&wgpu::TextureDescriptor {
            label: Some(label),
            size: wgpu::Extent3d { width: size.0, height: size.1, depth_or_array_layers: 1 },
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: HDR_FORMAT,
            usage: wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::TEXTURE_BINDING,
            view_formats: &[],
        })
        .create_view(&wgpu::TextureViewDescriptor::default())
}

#[derive(Clone, Copy)]
enum Slot {
    Tex,
    Samp,
    Uniform,
}
use Slot::{Samp, Tex, Uniform};

fn layout(device: &wgpu::Device, label: &str, slots: &[Slot]) -> wgpu::BindGroupLayout {
    let entries: Vec<_> = slots
        .iter()
        .enumerate()
        .map(|(i, slot)| wgpu::BindGroupLayoutEntry {
            binding: i as u32,
            visibility: wgpu::ShaderStages::FRAGMENT,
            ty: match slot {
                Tex => wgpu::BindingType::Texture {
                    sample_type: wgpu::TextureSampleType::Float { filterable: true },
                    view_dimension: wgpu::TextureViewDimension::D2,
                    multisampled: false,
                },
                Samp => wgpu::BindingType::Sampler(wgpu::SamplerBindingType::Filtering),
                Uniform => wgpu::BindingType::Buffer {
                    ty: wgpu::BufferBindingType::Uniform,
                    has_dynamic_offset: false,
                    min_binding_size: None,
                },
            },
            count: None,
        })
        .collect();
    device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
        label: Some(label),
        entries: &entries,
    })
}

enum Res<'a> {
    Tex(&'a wgpu::TextureView),
    Samp(&'a wgpu::Sampler),
    Buf(&'a wgpu::Buffer),
}

fn bind(device: &wgpu::Device, layout: &wgpu::BindGroupLayout, res: &[Res]) -> wgpu::BindGroup {
    let entries: Vec<_> = res
        .iter()
        .enumerate()
        .map(|(i, r)| wgpu::BindGroupEntry {
            binding: i as u32,
            resource: match r {
                Res::Tex(v) => wgpu::BindingResource::TextureView(v),
                Res::Samp(s) => wgpu::BindingResource::Sampler(s),
                Res::Buf(b) => b.as_entire_binding(),
            },
        })
        .collect();
    device.create_bind_group(&wgpu::BindGroupDescriptor {
        label: Some("post bind group"),
        layout,
        entries: &entries,
    })
}

fn fullscreen_pipeline(
    device: &wgpu::Device,
    label: &str,
    fragment: &str,
    bind_layout: &wgpu::BindGroupLayout,
    format: wgpu::TextureFormat,
    blend: Option<wgpu::BlendState>,
) -> wgpu::RenderPipeline {
    let source = format!("{}\n{}\n{}", shaders::FULLSCREEN, shaders::POST_UNIFORM, fragment);
    let shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
        label: Some(label),
        source: wgpu::ShaderSource::Wgsl(source.into()),
    });
    let layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
        label: Some(label),
        bind_group_layouts: &[Some(bind_layout)],
        ..Default::default()
    });
    device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
        label: Some(label),
        layout: Some(&layout),
        vertex: wgpu::VertexState {
            module: &shader,
            entry_point: Some("vs_main"),
            compilation_options: Default::default(),
            buffers: &[],
        },
        fragment: Some(wgpu::FragmentState {
            module: &shader,
            entry_point: Some("fs_main"),
            compilation_options: Default::default(),
            targets: &[Some(wgpu::ColorTargetState {
                format,
                blend,
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

fn light_pipeline(device: &wgpu::Device) -> wgpu::RenderPipeline {
    let shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
        label: Some("light shader"),
        source: wgpu::ShaderSource::Wgsl(shaders::LIGHT.into()),
    });
    let layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
        label: Some("light layout"),
        ..Default::default()
    });
    const ATTRS: [wgpu::VertexAttribute; 4] = wgpu::vertex_attr_array![
        0 => Float32x2, 1 => Float32, 2 => Float32, 3 => Float32x4
    ];
    device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
        label: Some("light pipeline"),
        layout: Some(&layout),
        vertex: wgpu::VertexState {
            module: &shader,
            entry_point: Some("vs_main"),
            compilation_options: Default::default(),
            buffers: &[Some(wgpu::VertexBufferLayout {
                array_stride: std::mem::size_of::<LightInstance>() as u64,
                step_mode: wgpu::VertexStepMode::Instance,
                attributes: &ATTRS,
            })],
        },
        fragment: Some(wgpu::FragmentState {
            module: &shader,
            entry_point: Some("fs_main"),
            compilation_options: Default::default(),
            targets: &[Some(wgpu::ColorTargetState {
                format: HDR_FORMAT,
                blend: Some(wgpu::BlendState::ADDITIVE),
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
