//! wgpu renderer for the Reign engine.
//!
//! Positions are in world pixels on a fixed 640x360 view. The game draws into
//! an offscreen target sized to the view times the largest whole-number scale
//! that fits the window (3x on 1080p, 4x on 1440p), then copies it to the
//! window, letterboxing the rest. Because the target already has screen
//! resolution, art can mix pixel densities: 1x tiles show as chunky pixels
//! while a 2x or 3x hero or showpiece prop keeps its finer detail, and the
//! camera can move by less than a world pixel. The target is HDR: lights,
//! bloom, light shafts and grading are applied as it's copied to the window
//! (see [`post`]).
//!
//! Menus and HUD go on the [`Layer::Overlay`], drawn over the finished
//! frame so lighting, bloom and grading leave them as drawn.
//!
//! With the `hooks` feature, a host such as an editor can send the finished
//! frame to a texture of its own instead of the window
//! ([`Renderer::set_offscreen`]) and then draw the window itself
//! ([`Renderer::present_with`]). Without it, [`Renderer::present`] always
//! draws to the window.

mod color;
mod fog;
mod post;
mod shaders;
mod texture_cache;

pub use color::Color;
pub use fog::{Fog, FogStyle, MAX_STIRS, Stir};
use post::{HDR_FORMAT, Post};
pub use post::{Light, Lighting, Shafts};
pub use texture_cache::TextureCache;
#[cfg(feature = "hooks")]
pub use wgpu;

/// How much of the world is visible, in world pixels. 1x art has one texel
/// per world pixel.
pub const VIEW_WIDTH: u32 = 640;
pub const VIEW_HEIGHT: u32 = 360;

/// Sprite textures: sRGB art, read back as linear.
const TEXTURE_FORMAT: wgpu::TextureFormat = wgpu::TextureFormat::Rgba8UnormSrgb;
const INITIAL_RECT_CAPACITY: usize = 1024;
const INITIAL_SPRITE_CAPACITY: usize = 64;

#[repr(C)]
#[derive(Clone, Copy, Debug, bytemuck::Pod, bytemuck::Zeroable)]
struct RectInstance {
    pos: [f32; 2],
    size: [f32; 2],
    color: [f32; 4],
}

#[repr(C)]
#[derive(Clone, Copy, Debug, bytemuck::Pod, bytemuck::Zeroable)]
struct SpriteInstance {
    pos: [f32; 2],
    size: [f32; 2],
    uv_min: [f32; 2],
    uv_max: [f32; 2],
    color: [f32; 4],
    crimson: [f32; 2],
}

/// A texture loaded with [`Renderer::load_texture`].
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct TextureId(usize);

struct Texture {
    size: (u32, u32),
    bind_group: wgpu::BindGroup,
}

/// How a draw combines with what's under it.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Blend {
    /// Covers what's under it by its alpha.
    #[default]
    Alpha,
    /// Adds its colour, scaled by its alpha: glows, flares, light.
    Add,
}

/// Which pass a draw lands in.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Layer {
    /// The lit HDR world: lights, bloom and grading apply.
    #[default]
    World,
    /// Over the finished frame, exactly as drawn: menus and HUD. Same world
    /// pixel coordinates; fog is not drawn here.
    Overlay,
}

/// A run of consecutive draws of one kind, kept in submission order so
/// sprites and rectangles layer the way they were queued.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Batch {
    Rects { blend: Blend, start: u32, end: u32 },
    Sprites { blend: Blend, texture: usize, start: u32, end: u32 },
    Fog { start: u32, end: u32 },
}

pub struct Renderer {
    surface: wgpu::Surface<'static>,
    device: wgpu::Device,
    queue: wgpu::Queue,
    config: wgpu::SurfaceConfiguration,
    target: Target,
    /// Indexed by [`Blend`].
    rect_pipelines: [wgpu::RenderPipeline; 2],
    rect_buffer: wgpu::Buffer,
    rect_capacity: usize,
    rects: Vec<RectInstance>,
    sprite_pipelines: [wgpu::RenderPipeline; 2],
    /// The same pipelines for the window's format, for the overlay.
    overlay_rect_pipelines: [wgpu::RenderPipeline; 2],
    overlay_sprite_pipelines: [wgpu::RenderPipeline; 2],
    sprite_buffer: wgpu::Buffer,
    sprite_capacity: usize,
    sprites: Vec<SpriteInstance>,
    fog: fog::FogPass,
    textures: Vec<Texture>,
    batches: Vec<Batch>,
    overlay: Vec<Batch>,
    layer: Layer,
    offset: [f32; 2],
    draw_scale: f32,
    blend: Blend,
    tex_layout: wgpu::BindGroupLayout,
    sampler: wgpu::Sampler,
    post: Post,
    /// Where [`Renderer::present`] draws instead of the window, if set.
    #[cfg(feature = "hooks")]
    offscreen: Option<Offscreen>,
}

/// A texture the whole frame (world, post and overlay) can be drawn into
/// instead of the window, in the window's format so the same pipelines
/// apply. Made by [`Renderer::create_offscreen`].
#[cfg(feature = "hooks")]
#[derive(Clone, Debug)]
pub struct Offscreen {
    pub texture: wgpu::Texture,
    pub view: wgpu::TextureView,
    /// Width and height in pixels.
    pub size: (u32, u32),
}

/// Offscreen HDR target at view size times `scale`.
struct Target {
    scale: u32,
    view: wgpu::TextureView,
}

impl Renderer {
    /// `target` is the window; `size` its current size in physical pixels.
    pub async fn new(
        target: impl Into<wgpu::SurfaceTarget<'static>>,
        size: (u32, u32),
    ) -> Result<Self, String> {
        let instance =
            wgpu::Instance::new(wgpu::InstanceDescriptor::new_without_display_handle_from_env());
        let surface = instance.create_surface(target).map_err(|e| e.to_string())?;
        let adapter = instance
            .request_adapter(&wgpu::RequestAdapterOptions {
                power_preference: wgpu::PowerPreference::HighPerformance,
                compatible_surface: Some(&surface),
                force_fallback_adapter: false,
                apply_limit_buckets: false,
            })
            .await
            .map_err(|e| e.to_string())?;
        log::info!("GPU: {:?}", adapter.get_info());
        let (device, queue) = adapter
            .request_device(&wgpu::DeviceDescriptor {
                label: Some("reign device"),
                ..Default::default()
            })
            .await
            .map_err(|e| e.to_string())?;

        let caps = surface.get_capabilities(&adapter);
        let format = caps.formats.iter().copied().find(|f| f.is_srgb()).unwrap_or(caps.formats[0]);
        let config = wgpu::SurfaceConfiguration {
            usage: wgpu::TextureUsages::RENDER_ATTACHMENT,
            format,
            width: size.0.max(1),
            height: size.1.max(1),
            // Vsync without the input lag of a deep queue.
            present_mode: wgpu::PresentMode::AutoVsync,
            desired_maximum_frame_latency: 1,
            alpha_mode: caps.alpha_modes[0],
            view_formats: vec![],
            color_space: wgpu::SurfaceColorSpace::Auto,
        };
        surface.configure(&device, &config);

        let rect_pipelines =
            [Blend::Alpha, Blend::Add].map(|b| create_rect_pipeline(&device, HDR_FORMAT, b));
        let overlay_rect_pipelines =
            [Blend::Alpha, Blend::Add].map(|b| create_rect_pipeline(&device, format, b));
        let rect_buffer = create_instance_buffer::<RectInstance>(&device, INITIAL_RECT_CAPACITY);
        let (tex_layout, sampler) = create_texture_binding(&device);
        let sprite_pipelines = [Blend::Alpha, Blend::Add]
            .map(|b| create_sprite_pipeline(&device, &tex_layout, HDR_FORMAT, b));
        let overlay_sprite_pipelines = [Blend::Alpha, Blend::Add]
            .map(|b| create_sprite_pipeline(&device, &tex_layout, format, b));
        let sprite_buffer =
            create_instance_buffer::<SpriteInstance>(&device, INITIAL_SPRITE_CAPACITY);
        let fog = fog::FogPass::new(&device, HDR_FORMAT);
        let target = create_target(&device, view_scale(config.width, config.height));
        let post = Post::new(&device, &tex_layout, &sampler, format, &target.view, target.size());

        Ok(Renderer {
            surface,
            device,
            queue,
            config,
            target,
            rect_pipelines,
            rect_buffer,
            rect_capacity: INITIAL_RECT_CAPACITY,
            rects: Vec::with_capacity(INITIAL_RECT_CAPACITY),
            sprite_pipelines,
            overlay_rect_pipelines,
            overlay_sprite_pipelines,
            sprite_buffer,
            sprite_capacity: INITIAL_SPRITE_CAPACITY,
            sprites: Vec::with_capacity(INITIAL_SPRITE_CAPACITY),
            fog,
            textures: Vec::new(),
            batches: Vec::new(),
            overlay: Vec::new(),
            layer: Layer::World,
            offset: [0.0, 0.0],
            draw_scale: 1.0,
            blend: Blend::Alpha,
            tex_layout,
            sampler,
            post,
            #[cfg(feature = "hooks")]
            offscreen: None,
        })
    }

    pub fn resize(&mut self, width: u32, height: u32) {
        if width == 0 || height == 0 {
            return;
        }
        self.config.width = width;
        self.config.height = height;
        self.surface.configure(&self.device, &self.config);
        self.fit_target();
    }

    /// The size of what [`Renderer::present`] draws into: the window, or the
    /// offscreen texture when one is set.
    fn output_size(&self) -> (u32, u32) {
        #[cfg(feature = "hooks")]
        if let Some(offscreen) = &self.offscreen {
            return offscreen.size;
        }
        (self.config.width, self.config.height)
    }

    /// Resizes the HDR target to the output's whole-number view scale.
    fn fit_target(&mut self) {
        let (width, height) = self.output_size();
        let scale = view_scale(width, height);
        if scale != self.target.scale {
            self.target = create_target(&self.device, scale);
            self.post.resize(&self.device, &self.sampler, &self.target.view, self.target.size());
        }
    }

    /// World pixels to screen pixels for the current window. Art at density
    /// `d` lands on whole screen pixels when this is a multiple of `d`.
    pub fn scale(&self) -> u32 {
        self.target.scale
    }

    /// Queues a solid rectangle in world pixels, origin top-left. Fractional
    /// positions are honoured down to one screen pixel.
    pub fn rect(&mut self, x: f32, y: f32, w: f32, h: f32, color: Color) {
        let pos = [(x + self.offset[0]) * self.draw_scale, (y + self.offset[1]) * self.draw_scale];
        self.rects.push(RectInstance {
            pos,
            size: [w * self.draw_scale, h * self.draw_scale],
            color: color.to_linear(),
        });
        let end = self.rects.len() as u32;
        let blend = self.blend;
        let batches = self.layer_batches();
        match batches.last_mut() {
            Some(Batch::Rects { blend: b, end: last, .. }) if *b == blend => *last = end,
            _ => batches.push(Batch::Rects { blend, start: end - 1, end }),
        }
    }

    /// Uploads an sRGB RGBA8 image for [`Renderer::sprite`]. Sampled with
    /// nearest filtering, like everything else.
    pub fn load_texture(&mut self, rgba: &[u8], width: u32, height: u32) -> TextureId {
        let texture = self.upload(rgba, width, height);
        self.textures.push(texture);
        TextureId(self.textures.len() - 1)
    }

    /// Replaces a loaded texture's pixels (and size) in place, so every draw
    /// using `id` shows the new image: for hot reloading art.
    pub fn replace_texture(&mut self, id: TextureId, rgba: &[u8], width: u32, height: u32) {
        let texture = self.upload(rgba, width, height);
        if let Some(slot) = self.textures.get_mut(id.0) {
            *slot = texture;
        }
    }

    /// A loaded texture's size in texels.
    pub fn texture_size(&self, id: TextureId) -> Option<(u32, u32)> {
        self.textures.get(id.0).map(|t| t.size)
    }

    fn upload(&mut self, rgba: &[u8], width: u32, height: u32) -> Texture {
        assert_eq!(rgba.len(), (width * height * 4) as usize, "RGBA8 data size");
        let size = wgpu::Extent3d { width, height, depth_or_array_layers: 1 };
        let texture = self.device.create_texture(&wgpu::TextureDescriptor {
            label: Some("sprite texture"),
            size,
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: TEXTURE_FORMAT,
            usage: wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_DST,
            view_formats: &[],
        });
        self.queue.write_texture(
            wgpu::TexelCopyTextureInfo {
                texture: &texture,
                mip_level: 0,
                origin: wgpu::Origin3d::ZERO,
                aspect: wgpu::TextureAspect::All,
            },
            rgba,
            wgpu::TexelCopyBufferLayout {
                offset: 0,
                bytes_per_row: Some(width * 4),
                rows_per_image: Some(height),
            },
            size,
        );
        let view = texture.create_view(&wgpu::TextureViewDescriptor::default());
        let bind_group = bind_texture(&self.device, &self.tex_layout, &view, &self.sampler);
        Texture { size: (width, height), bind_group }
    }

    /// Queues the texel rectangle `src` (x, y, w, h) of `texture`, drawn at
    /// world rectangle (x, y, w, h). A texel can cover less than a world
    /// pixel, so sprites can carry finer detail than the room. `flip`
    /// mirrors it horizontally; `tint` multiplies its colours.
    #[allow(clippy::too_many_arguments)]
    pub fn sprite(
        &mut self,
        texture: TextureId,
        src: [f32; 4],
        x: f32,
        y: f32,
        w: f32,
        h: f32,
        flip: bool,
        tint: Color,
    ) {
        let (tw, th) = self.textures[texture.0].size;
        let (u0, u1) = (src[0] / tw as f32, (src[0] + src[2]) / tw as f32);
        let (v0, v1) = (src[1] / th as f32, (src[1] + src[3]) / th as f32);
        let (u0, u1) = if flip { (u1, u0) } else { (u0, u1) };
        self.sprites.push(SpriteInstance {
            pos: [(x + self.offset[0]) * self.draw_scale, (y + self.offset[1]) * self.draw_scale],
            size: [w * self.draw_scale, h * self.draw_scale],
            uv_min: [u0, v0],
            uv_max: [u1, v1],
            color: tint.to_linear(),
            crimson: [0.0; 2],
        });
        let end = self.sprites.len() as u32;
        let blend = self.blend;
        let batches = self.layer_batches();
        match batches.last_mut() {
            Some(Batch::Sprites { blend: b, texture: t, end: last, .. })
                if *t == texture.0 && *b == blend =>
            {
                *last = end
            }
            _ => batches.push(Batch::Sprites { blend, texture: texture.0, start: end - 1, end }),
        }
    }

    /// Draws a sprite with animated crimson energy clipped to its silhouette.
    /// Strength fades the effect without replacing the underlying art.
    #[allow(clippy::too_many_arguments)]
    pub fn sprite_crimson(
        &mut self,
        texture: TextureId,
        src: [f32; 4],
        x: f32,
        y: f32,
        w: f32,
        h: f32,
        tint: Color,
        time: f32,
        strength: f32,
    ) {
        self.sprite(texture, src, x, y, w, h, false, tint);
        self.sprites.last_mut().unwrap().crimson = [time, strength.clamp(0.0, 1.0)];
    }

    /// Queues a fog volume, shifted by the current offset. It layers with
    /// everything else in the order queued. World layer only.
    pub fn fog(&mut self, fog: Fog) {
        if self.layer == Layer::Overlay {
            return;
        }
        let s = self.draw_scale;
        let fog = Fog {
            x: fog.x * s,
            y: fog.y * s,
            w: fog.w * s,
            h: fog.h * s,
            scale: fog.scale * s,
            drift: (fog.drift.0 * s, fog.drift.1 * s),
            ..fog
        };
        let end = self.fog.push(fog, self.offset.map(|v| v * s));
        match self.batches.last_mut() {
            Some(Batch::Fog { end: last, .. }) => *last = end,
            _ => self.batches.push(Batch::Fog { start: end - 1, end }),
        }
    }

    /// Disturbs this frame's fog around a point, shifted by the current
    /// offset. Up to [`MAX_STIRS`] a frame; the rest are dropped.
    pub fn stir(&mut self, stir: Stir) {
        let s = self.draw_scale;
        let stir = Stir {
            x: stir.x * s,
            y: stir.y * s,
            radius: stir.radius * s,
            push: (stir.push.0 * s, stir.push.1 * s),
            swirl: stir.swirl * s,
            ..stir
        };
        self.fog.stir(stir, self.offset.map(|v| v * s));
    }

    /// Which pass everything queued after this call lands in. Back to
    /// [`Layer::World`] each frame.
    pub fn set_layer(&mut self, layer: Layer) {
        self.layer = layer;
    }

    fn layer_batches(&mut self) -> &mut Vec<Batch> {
        match self.layer {
            Layer::World => &mut self.batches,
            Layer::Overlay => &mut self.overlay,
        }
    }

    /// Waits for the display's refresh to present when on, else presents as
    /// soon as a frame is ready (tearing is possible).
    pub fn set_vsync(&mut self, on: bool) {
        let mode = if on { wgpu::PresentMode::AutoVsync } else { wgpu::PresentMode::AutoNoVsync };
        if self.config.present_mode != mode {
            self.config.present_mode = mode;
            self.surface.configure(&self.device, &self.config);
        }
    }

    /// How everything queued after this call blends.
    pub fn set_blend(&mut self, blend: Blend) {
        self.blend = blend;
    }

    /// Queues a light for this frame, shifted by the current offset.
    pub fn light(&mut self, light: Light) {
        let s = self.draw_scale;
        let light = Light { x: light.x * s, y: light.y * s, radius: light.radius * s, ..light };
        self.post.push_light(light, self.offset.map(|v| v * s));
    }

    /// Sets how frames are lit and graded from now on.
    pub fn set_lighting(&mut self, lighting: Lighting) {
        self.post.lighting = lighting;
    }

    /// Shifts everything queued after this call, in world pixels. For
    /// screen shake now, and the camera later.
    pub fn set_offset(&mut self, x: f32, y: f32) {
        self.offset = [x, y];
    }

    /// Scales subsequent draw coordinates, sizes, lights and fog after the
    /// offset is applied. Reset to 1 each frame. Used by the editor camera.
    pub fn set_draw_scale(&mut self, scale: f32) {
        assert!(scale.is_finite() && scale > 0.0);
        self.draw_scale = scale;
    }

    /// Draws everything queued this frame and presents it.
    pub fn present(&mut self, clear: Color) {
        #[cfg(feature = "hooks")]
        if let Some(offscreen) = &self.offscreen {
            let (view, size) = (offscreen.view.clone(), offscreen.size);
            self.render_frame(clear, &view, size);
            return;
        }
        let Some(frame) = self.acquire() else {
            self.clear_queue();
            return;
        };
        let frame_view = frame.texture.create_view(&wgpu::TextureViewDescriptor::default());
        self.render_frame(clear, &frame_view, (self.config.width, self.config.height));
        self.queue.present(frame);
    }

    /// The window's next texture, or `None` to skip this frame.
    fn acquire(&mut self) -> Option<wgpu::SurfaceTexture> {
        match self.surface.get_current_texture() {
            wgpu::CurrentSurfaceTexture::Success(frame) => Some(frame),
            wgpu::CurrentSurfaceTexture::Suboptimal(frame) => {
                self.surface.configure(&self.device, &self.config);
                Some(frame)
            }
            wgpu::CurrentSurfaceTexture::Outdated | wgpu::CurrentSurfaceTexture::Lost => {
                self.surface.configure(&self.device, &self.config);
                None
            }
            other => {
                log::debug!("skipping frame: {other:?}");
                None
            }
        }
    }

    /// Draws the queued world, post chain and overlay into `view`, `size`
    /// pixels, letterboxed at a whole-number scale, and submits it.
    fn render_frame(&mut self, clear: Color, view: &wgpu::TextureView, size: (u32, u32)) {
        if self.rects.len() > self.rect_capacity {
            self.rect_capacity = self.rects.len().next_power_of_two();
            self.rect_buffer =
                create_instance_buffer::<RectInstance>(&self.device, self.rect_capacity);
        }
        self.queue.write_buffer(&self.rect_buffer, 0, bytemuck::cast_slice(&self.rects));
        if self.sprites.len() > self.sprite_capacity {
            self.sprite_capacity = self.sprites.len().next_power_of_two();
            self.sprite_buffer =
                create_instance_buffer::<SpriteInstance>(&self.device, self.sprite_capacity);
        }
        self.queue.write_buffer(&self.sprite_buffer, 0, bytemuck::cast_slice(&self.sprites));
        self.fog.upload(&self.device, &self.queue);

        let mut encoder = self
            .device
            .create_command_encoder(&wgpu::CommandEncoderDescriptor { label: Some("frame") });
        {
            let mut pass = begin_pass(&mut encoder, &self.target.view, Some(clear), "world pass");
            self.draw_batches(&mut pass, &self.batches, false);
        }
        let viewport = integer_viewport(size.0, size.1);
        self.post.render(&self.device, &self.queue, &mut encoder, view, viewport);
        if !self.overlay.is_empty() {
            let mut pass = begin_pass(&mut encoder, view, None, "overlay pass");
            let (x, y, w, h) = viewport;
            pass.set_viewport(x as f32, y as f32, w as f32, h as f32, 0.0, 1.0);
            self.draw_batches(&mut pass, &self.overlay, true);
        }
        self.queue.submit([encoder.finish()]);
        self.clear_queue();
    }

    fn draw_batches(&self, pass: &mut wgpu::RenderPass, batches: &[Batch], overlay: bool) {
        let (rects, sprites) = if overlay {
            (&self.overlay_rect_pipelines, &self.overlay_sprite_pipelines)
        } else {
            (&self.rect_pipelines, &self.sprite_pipelines)
        };
        for batch in batches {
            match *batch {
                Batch::Rects { blend, start, end } => {
                    pass.set_pipeline(&rects[blend as usize]);
                    pass.set_vertex_buffer(0, self.rect_buffer.slice(..));
                    pass.draw(0..6, start..end);
                }
                Batch::Sprites { blend, texture, start, end } => {
                    pass.set_pipeline(&sprites[blend as usize]);
                    pass.set_bind_group(0, &self.textures[texture].bind_group, &[]);
                    pass.set_vertex_buffer(0, self.sprite_buffer.slice(..));
                    pass.draw(0..6, start..end);
                }
                Batch::Fog { start, end } => self.fog.draw(pass, start, end),
            }
        }
    }

    fn clear_queue(&mut self) {
        self.draw_scale = 1.0;
        self.blend = Blend::Alpha;
        self.layer = Layer::World;
        self.overlay.clear();
        self.post.clear_lights();
        self.rects.clear();
        self.sprites.clear();
        self.fog.clear();
        self.batches.clear();
    }
}

/// Access for hosts that draw the window themselves, such as an editor.
#[cfg(feature = "hooks")]
impl Renderer {
    pub fn device(&self) -> &wgpu::Device {
        &self.device
    }

    pub fn queue(&self) -> &wgpu::Queue {
        &self.queue
    }

    /// The window's texture format; offscreen frames use it too.
    pub fn surface_format(&self) -> wgpu::TextureFormat {
        self.config.format
    }

    /// The window's size in physical pixels.
    pub fn surface_size(&self) -> (u32, u32) {
        (self.config.width, self.config.height)
    }

    /// A texture to draw whole frames into, `width` by `height` pixels.
    /// It can also be sampled, to show the frame inside another UI.
    pub fn create_offscreen(&self, width: u32, height: u32) -> Offscreen {
        let size = (width.max(1), height.max(1));
        let texture = self.device.create_texture(&wgpu::TextureDescriptor {
            label: Some("offscreen frame"),
            size: wgpu::Extent3d { width: size.0, height: size.1, depth_or_array_layers: 1 },
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: self.config.format,
            usage: wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::TEXTURE_BINDING,
            view_formats: &[],
        });
        let view = texture.create_view(&wgpu::TextureViewDescriptor::default());
        Offscreen { texture, view, size }
    }

    /// Sends [`Renderer::present`] into `offscreen` instead of the window,
    /// letterboxed at a whole-number scale of its size, or back to the
    /// window with `None`. The window is then only drawn by
    /// [`Renderer::present_with`].
    pub fn set_offscreen(&mut self, offscreen: Option<Offscreen>) {
        self.offscreen = offscreen;
        self.fit_target();
    }

    pub fn offscreen(&self) -> Option<&Offscreen> {
        self.offscreen.as_ref()
    }

    /// Draws the window with `draw`, at its full resolution, and presents
    /// it. `draw` records into the encoder (the window texture and its size
    /// are given) and returns any command buffers that must be submitted
    /// before it, such as a UI library's buffer uploads.
    pub fn present_with(
        &mut self,
        draw: impl FnOnce(
            &wgpu::Device,
            &wgpu::Queue,
            &mut wgpu::CommandEncoder,
            &wgpu::TextureView,
            (u32, u32),
        ) -> Vec<wgpu::CommandBuffer>,
    ) {
        let Some(frame) = self.acquire() else { return };
        let view = frame.texture.create_view(&wgpu::TextureViewDescriptor::default());
        let mut encoder = self
            .device
            .create_command_encoder(&wgpu::CommandEncoderDescriptor { label: Some("host frame") });
        let size = (self.config.width, self.config.height);
        let mut buffers = draw(&self.device, &self.queue, &mut encoder, &view, size);
        buffers.push(encoder.finish());
        self.queue.submit(buffers);
        self.queue.present(frame);
    }
}

/// Largest whole-number scale of the view that fits the window, at least 1.
pub fn view_scale(width: u32, height: u32) -> u32 {
    (width / VIEW_WIDTH).min(height / VIEW_HEIGHT).max(1)
}

/// Where the target lands in the window, centred. Falls back to a fractional
/// fit when the window is smaller than the view.
pub fn integer_viewport(width: u32, height: u32) -> (u32, u32, u32, u32) {
    let scale = (width / VIEW_WIDTH).min(height / VIEW_HEIGHT);
    let (w, h) = if scale >= 1 {
        (VIEW_WIDTH * scale, VIEW_HEIGHT * scale)
    } else {
        let fit = (width as f32 / VIEW_WIDTH as f32).min(height as f32 / VIEW_HEIGHT as f32);
        (((VIEW_WIDTH as f32 * fit) as u32).max(1), ((VIEW_HEIGHT as f32 * fit) as u32).max(1))
    };
    ((width - w) / 2, (height - h) / 2, w, h)
}

fn create_target(device: &wgpu::Device, scale: u32) -> Target {
    let view = post::hdr_target(device, "world target", (VIEW_WIDTH * scale, VIEW_HEIGHT * scale));
    Target { scale, view }
}

impl Target {
    fn size(&self) -> (u32, u32) {
        (VIEW_WIDTH * self.scale, VIEW_HEIGHT * self.scale)
    }
}

/// A texture and the pixel sampler, for the upscale and sprite shaders.
pub(crate) fn bind_texture(
    device: &wgpu::Device,
    layout: &wgpu::BindGroupLayout,
    view: &wgpu::TextureView,
    sampler: &wgpu::Sampler,
) -> wgpu::BindGroup {
    device.create_bind_group(&wgpu::BindGroupDescriptor {
        label: Some("texture bind group"),
        layout,
        entries: &[
            wgpu::BindGroupEntry { binding: 0, resource: wgpu::BindingResource::TextureView(view) },
            wgpu::BindGroupEntry { binding: 1, resource: wgpu::BindingResource::Sampler(sampler) },
        ],
    })
}

/// A pass drawing into `view`, cleared to `clear` first, or drawing over
/// what's there with `None`.
pub(crate) fn begin_pass<'e>(
    encoder: &'e mut wgpu::CommandEncoder,
    view: &wgpu::TextureView,
    clear: Option<Color>,
    label: &str,
) -> wgpu::RenderPass<'e> {
    let load = match clear.map(Color::to_linear) {
        Some([r, g, b, a]) => {
            wgpu::LoadOp::Clear(wgpu::Color { r: r as f64, g: g as f64, b: b as f64, a: a as f64 })
        }
        None => wgpu::LoadOp::Load,
    };
    encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
        label: Some(label),
        color_attachments: &[Some(wgpu::RenderPassColorAttachment {
            view,
            depth_slice: None,
            resolve_target: None,
            ops: wgpu::Operations { load, store: wgpu::StoreOp::Store },
        })],
        depth_stencil_attachment: None,
        timestamp_writes: None,
        occlusion_query_set: None,
        multiview_mask: None,
    })
}

pub(crate) fn create_instance_buffer<T>(device: &wgpu::Device, capacity: usize) -> wgpu::Buffer {
    device.create_buffer(&wgpu::BufferDescriptor {
        label: Some("instances"),
        size: (capacity * std::mem::size_of::<T>()) as u64,
        usage: wgpu::BufferUsages::VERTEX | wgpu::BufferUsages::COPY_DST,
        mapped_at_creation: false,
    })
}

fn create_rect_pipeline(
    device: &wgpu::Device,
    format: wgpu::TextureFormat,
    blend: Blend,
) -> wgpu::RenderPipeline {
    let shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
        label: Some("rect shader"),
        source: wgpu::ShaderSource::Wgsl(shaders::RECT.into()),
    });
    let layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
        label: Some("rect layout"),
        ..Default::default()
    });
    const ATTRS: [wgpu::VertexAttribute; 3] =
        wgpu::vertex_attr_array![0 => Float32x2, 1 => Float32x2, 2 => Float32x4];
    device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
        label: Some("rect pipeline"),
        layout: Some(&layout),
        vertex: wgpu::VertexState {
            module: &shader,
            entry_point: Some("vs_main"),
            compilation_options: Default::default(),
            buffers: &[Some(wgpu::VertexBufferLayout {
                array_stride: std::mem::size_of::<RectInstance>() as u64,
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
                blend: Some(blend_state(blend)),
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

fn create_sprite_pipeline(
    device: &wgpu::Device,
    texture_layout: &wgpu::BindGroupLayout,
    format: wgpu::TextureFormat,
    blend: Blend,
) -> wgpu::RenderPipeline {
    let shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
        label: Some("sprite shader"),
        source: wgpu::ShaderSource::Wgsl(shaders::SPRITE.into()),
    });
    let layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
        label: Some("sprite layout"),
        bind_group_layouts: &[Some(texture_layout)],
        ..Default::default()
    });
    const ATTRS: [wgpu::VertexAttribute; 6] = wgpu::vertex_attr_array![
        0 => Float32x2, 1 => Float32x2, 2 => Float32x2, 3 => Float32x2, 4 => Float32x4, 5 => Float32x2
    ];
    device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
        label: Some("sprite pipeline"),
        layout: Some(&layout),
        vertex: wgpu::VertexState {
            module: &shader,
            entry_point: Some("vs_main"),
            compilation_options: Default::default(),
            buffers: &[Some(wgpu::VertexBufferLayout {
                array_stride: std::mem::size_of::<SpriteInstance>() as u64,
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
                blend: Some(blend_state(blend)),
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

fn blend_state(blend: Blend) -> wgpu::BlendState {
    match blend {
        Blend::Alpha => wgpu::BlendState::ALPHA_BLENDING,
        Blend::Add => wgpu::BlendState {
            color: wgpu::BlendComponent {
                src_factor: wgpu::BlendFactor::SrcAlpha,
                dst_factor: wgpu::BlendFactor::One,
                operation: wgpu::BlendOperation::Add,
            },
            alpha: wgpu::BlendComponent {
                src_factor: wgpu::BlendFactor::Zero,
                dst_factor: wgpu::BlendFactor::One,
                operation: wgpu::BlendOperation::Add,
            },
        },
    }
}

/// The layout for one texture and a sampler, and the pixel sampler.
fn create_texture_binding(device: &wgpu::Device) -> (wgpu::BindGroupLayout, wgpu::Sampler) {
    let layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
        label: Some("texture bindings"),
        entries: &[
            wgpu::BindGroupLayoutEntry {
                binding: 0,
                visibility: wgpu::ShaderStages::FRAGMENT,
                ty: wgpu::BindingType::Texture {
                    sample_type: wgpu::TextureSampleType::Float { filterable: true },
                    view_dimension: wgpu::TextureViewDimension::D2,
                    multisampled: false,
                },
                count: None,
            },
            wgpu::BindGroupLayoutEntry {
                binding: 1,
                visibility: wgpu::ShaderStages::FRAGMENT,
                ty: wgpu::BindingType::Sampler(wgpu::SamplerBindingType::Filtering),
                count: None,
            },
        ],
    });
    // The target is already at screen scale, so art and the final copy are
    // sampled nearest to keep edges hard.
    let sampler = device.create_sampler(&wgpu::SamplerDescriptor {
        label: Some("pixel sampler"),
        mag_filter: wgpu::FilterMode::Nearest,
        min_filter: wgpu::FilterMode::Nearest,
        ..Default::default()
    });
    (layout, sampler)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn crimson_sprite_shader_validates_for_world_and_overlay_pipelines() {
        pollster::block_on(async {
            let instance = wgpu::Instance::new(
                wgpu::InstanceDescriptor::new_without_display_handle_from_env(),
            );
            let Ok(adapter) =
                instance.request_adapter(&wgpu::RequestAdapterOptions::default()).await
            else {
                eprintln!("No GPU adapter available for sprite pipeline validation");
                return;
            };
            let (device, _) =
                adapter.request_device(&wgpu::DeviceDescriptor::default()).await.unwrap();
            let scope = device.push_error_scope(wgpu::ErrorFilter::Validation);
            let (layout, _) = create_texture_binding(&device);
            for format in [HDR_FORMAT, wgpu::TextureFormat::Rgba8UnormSrgb] {
                for blend in [Blend::Alpha, Blend::Add] {
                    let _ = create_sprite_pipeline(&device, &layout, format, blend);
                }
            }
            assert!(scope.pop().await.is_none(), "sprite shader or instance layout is invalid");
        });
    }

    #[test]
    fn viewport_uses_whole_number_scale() {
        assert_eq!(integer_viewport(1920, 1080), (0, 0, 1920, 1080));
        assert_eq!(integer_viewport(2560, 1440), (0, 0, 2560, 1440));
        // 1440x900 fits 2x (1280x720), letterboxed.
        assert_eq!(integer_viewport(1440, 900), (80, 90, 1280, 720));
    }

    #[test]
    fn viewport_shrinks_below_view_size() {
        let (_, _, w, h) = integer_viewport(320, 180);
        assert_eq!((w, h), (320, 180));
    }

    #[test]
    fn target_scale_matches_common_screens() {
        assert_eq!(view_scale(1280, 720), 2);
        assert_eq!(view_scale(1920, 1080), 3);
        assert_eq!(view_scale(2560, 1440), 4);
        assert_eq!(view_scale(3840, 2160), 6);
        assert_eq!(view_scale(600, 300), 1);
    }
}
