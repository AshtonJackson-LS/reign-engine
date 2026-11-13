//! Fog: drifting volumes of mist drawn into the world like any other layer,
//! so they can sit between parallax layers, behind the room or in front of
//! it. Each is a rectangle filled with layered noise that drifts and churns
//! over time, thinning out towards its edges. Fog lands in the HDR world
//! before the light map is applied, so lamps and the moon light it up and
//! bloom off it, and it sinks into shadow away from them.
//!
//! It comes in two styles: [`FogStyle::Pixel`] snaps to the world pixel grid
//! and dithers, to sit in with the art, and [`FogStyle::Volumetric`] is
//! smooth at full screen resolution, shaded thicker in its cores and
//! brighter at its thin edges, laid over the art like real air.
//!
//! [`Stir`]s disturb it: each parts the fog around a point, pushes it and
//! swirls it, so things moving through it leave a wake.

use crate::Color;

/// How a fog volume is drawn.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum FogStyle {
    /// On the world pixel grid, in dithered steps of opacity.
    #[default]
    Pixel,
    /// Smooth at screen resolution, with soft self-shadowing.
    Volumetric,
}

/// One fog volume, in world pixels.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Fog {
    pub x: f32,
    pub y: f32,
    pub w: f32,
    pub h: f32,
    pub color: Color,
    /// Opacity at the thickest point, 0..1.
    pub density: f32,
    /// Size of the biggest wisps, in world pixels.
    pub scale: f32,
    /// How fast the fog drifts, in world pixels per second.
    pub drift: (f32, f32),
    /// Seconds; with `drift`, how far it has moved.
    pub time: f32,
    /// Picks a different pattern for each volume.
    pub seed: f32,
    /// How much of the height, from the top and from the bottom, fades in.
    /// Ground mist fades out towards the top and stays thick at the bottom.
    pub feather: (f32, f32),
    pub style: FogStyle,
    /// How much [`Stir`]s move this volume, 0..1: far haze barely notices
    /// the player, mist at their feet curls round them.
    pub stir: f32,
}

/// A disturbance in the fog, in world pixels, shifted by the renderer's
/// offset like everything else. It fades out over `radius`.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Stir {
    pub x: f32,
    pub y: f32,
    pub radius: f32,
    /// How much it clears the fog at its centre, 0..1.
    pub part: f32,
    /// How far it shoves the fog, in pixels.
    pub push: (f32, f32),
    /// How far it twists the fog round its centre, in pixels; negative
    /// turns the other way.
    pub swirl: f32,
}

/// Stirs past this many in a frame are dropped.
pub const MAX_STIRS: usize = 24;

#[repr(C)]
#[derive(Clone, Copy, Debug, bytemuck::Pod, bytemuck::Zeroable)]
pub(crate) struct FogInstance {
    pos: [f32; 2],
    size: [f32; 2],
    color: [f32; 4],
    /// Density, scale, drift x, drift y.
    shape: [f32; 4],
    /// Time, seed, feather top, feather bottom.
    motion: [f32; 4],
    /// Style (0 pixel, 1 volumetric), stir response, unused, unused.
    style: [f32; 4],
}

impl FogInstance {
    fn new(fog: Fog, offset: [f32; 2]) -> Self {
        let style = match fog.style {
            FogStyle::Pixel => 0.0,
            FogStyle::Volumetric => 1.0,
        };
        FogInstance {
            pos: [fog.x + offset[0], fog.y + offset[1]],
            size: [fog.w, fog.h],
            color: fog.color.to_linear(),
            shape: [fog.density, fog.scale.max(1.0), fog.drift.0, fog.drift.1],
            motion: [fog.time, fog.seed, fog.feather.0, fog.feather.1],
            style: [style, fog.stir, 0.0, 0.0],
        }
    }
}

#[repr(C)]
#[derive(Clone, Copy, Debug, bytemuck::Pod, bytemuck::Zeroable)]
struct StirUniform {
    /// Count, then padding.
    count: [u32; 4],
    /// Per stir: x, y, radius, part.
    at: [[f32; 4]; MAX_STIRS],
    /// Per stir: push x, push y, swirl, unused.
    shove: [[f32; 4]; MAX_STIRS],
}

/// Everything the renderer needs to draw fog.
pub(crate) struct FogPass {
    pipeline: wgpu::RenderPipeline,
    buffer: wgpu::Buffer,
    capacity: usize,
    instances: Vec<FogInstance>,
    stirs: Vec<Stir>,
    stir_buffer: wgpu::Buffer,
    bind_group: wgpu::BindGroup,
}

const INITIAL_CAPACITY: usize = 16;

impl FogPass {
    pub fn new(device: &wgpu::Device, format: wgpu::TextureFormat) -> Self {
        let layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("fog stirs layout"),
            entries: &[wgpu::BindGroupLayoutEntry {
                binding: 0,
                visibility: wgpu::ShaderStages::FRAGMENT,
                ty: wgpu::BindingType::Buffer {
                    ty: wgpu::BufferBindingType::Uniform,
                    has_dynamic_offset: false,
                    min_binding_size: None,
                },
                count: None,
            }],
        });
        let stir_buffer = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("fog stirs"),
            size: std::mem::size_of::<StirUniform>() as u64,
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        let bind_group = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("fog stirs"),
            layout: &layout,
            entries: &[wgpu::BindGroupEntry {
                binding: 0,
                resource: stir_buffer.as_entire_binding(),
            }],
        });
        FogPass {
            pipeline: create_pipeline(device, &layout, format),
            buffer: crate::create_instance_buffer::<FogInstance>(device, INITIAL_CAPACITY),
            capacity: INITIAL_CAPACITY,
            instances: Vec::with_capacity(INITIAL_CAPACITY),
            stirs: Vec::with_capacity(MAX_STIRS),
            stir_buffer,
            bind_group,
        }
    }

    /// Queues a volume; returns how many are queued.
    pub fn push(&mut self, fog: Fog, offset: [f32; 2]) -> u32 {
        self.instances.push(FogInstance::new(fog, offset));
        self.instances.len() as u32
    }

    pub fn stir(&mut self, stir: Stir, offset: [f32; 2]) {
        if self.stirs.len() < MAX_STIRS && stir.radius > 0.0 {
            self.stirs.push(Stir { x: stir.x + offset[0], y: stir.y + offset[1], ..stir });
        }
    }

    pub fn upload(&mut self, device: &wgpu::Device, queue: &wgpu::Queue) {
        if self.instances.len() > self.capacity {
            self.capacity = self.instances.len().next_power_of_two();
            self.buffer = crate::create_instance_buffer::<FogInstance>(device, self.capacity);
        }
        queue.write_buffer(&self.buffer, 0, bytemuck::cast_slice(&self.instances));
        let mut u = StirUniform {
            count: [self.stirs.len() as u32, 0, 0, 0],
            at: [[0.0; 4]; MAX_STIRS],
            shove: [[0.0; 4]; MAX_STIRS],
        };
        for (i, s) in self.stirs.iter().enumerate() {
            u.at[i] = [s.x, s.y, s.radius, s.part];
            u.shove[i] = [s.push.0, s.push.1, s.swirl, 0.0];
        }
        queue.write_buffer(&self.stir_buffer, 0, bytemuck::bytes_of(&u));
    }

    pub fn draw(&self, pass: &mut wgpu::RenderPass, start: u32, end: u32) {
        pass.set_pipeline(&self.pipeline);
        pass.set_bind_group(0, &self.bind_group, &[]);
        pass.set_vertex_buffer(0, self.buffer.slice(..));
        pass.draw(0..6, start..end);
    }

    pub fn clear(&mut self) {
        self.instances.clear();
        self.stirs.clear();
    }
}

fn create_pipeline(
    device: &wgpu::Device,
    stirs: &wgpu::BindGroupLayout,
    format: wgpu::TextureFormat,
) -> wgpu::RenderPipeline {
    let shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
        label: Some("fog shader"),
        source: wgpu::ShaderSource::Wgsl(crate::shaders::FOG.into()),
    });
    let layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
        label: Some("fog layout"),
        bind_group_layouts: &[Some(stirs)],
        ..Default::default()
    });
    const ATTRS: [wgpu::VertexAttribute; 6] = wgpu::vertex_attr_array![
        0 => Float32x2, 1 => Float32x2, 2 => Float32x4, 3 => Float32x4, 4 => Float32x4,
        5 => Float32x4
    ];
    device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
        label: Some("fog pipeline"),
        layout: Some(&layout),
        vertex: wgpu::VertexState {
            module: &shader,
            entry_point: Some("vs_main"),
            compilation_options: Default::default(),
            buffers: &[Some(wgpu::VertexBufferLayout {
                array_stride: std::mem::size_of::<FogInstance>() as u64,
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
                blend: Some(wgpu::BlendState::ALPHA_BLENDING),
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
