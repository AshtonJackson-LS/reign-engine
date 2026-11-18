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
//! (see the `post` module).
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
/// Normal maps and light masks are data, not colours: read back as stored.
const MATERIAL_FORMAT: wgpu::TextureFormat = wgpu::TextureFormat::Rgba8Unorm;
const INITIAL_RECT_CAPACITY: usize = 1024;
const INITIAL_SPRITE_CAPACITY: usize = 64;

#[repr(C)]
#[derive(Clone, Copy, Debug, bytemuck::Pod, bytemuck::Zeroable)]
struct RectInstance {
    pos: [f32; 2],
    /// The edge from `pos` to the next corner along x, and along y. A plain
    /// rectangle is `(w, 0)` and `(0, h)`; a line or a trail segment is a
    /// parallelogram with the same two edges turned.
    edge_x: [f32; 2],
    edge_y: [f32; 2],
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
    /// The normal map and light mask packed into one image of the same size,
    /// from [`Renderer::set_material`].
    material: Option<wgpu::BindGroup>,
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
    /// Draws the sprites again into the material target.
    material_pipeline: wgpu::RenderPipeline,
    /// The material of a sprite with none: it lights the old way.
    default_material: wgpu::BindGroup,
    /// How many textures have a material; the material pass is skipped at 0.
    materials: usize,
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

/// Offscreen HDR target at view size times `scale`, and beside it the
/// material target of the same size: what lights read to shade sprites with
/// normal maps (see [`Renderer::set_material`]).
struct Target {
    scale: u32,
    view: wgpu::TextureView,
    material: wgpu::TextureView,
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
        let material_pipeline = create_material_pipeline(&device, &tex_layout);
        let default_material = texture_bind(
            &device,
            &queue,
            &tex_layout,
            &sampler,
            &[0, 0, 0, 0],
            (1, 1),
            MATERIAL_FORMAT,
        );
        let overlay_sprite_pipelines = [Blend::Alpha, Blend::Add]
            .map(|b| create_sprite_pipeline(&device, &tex_layout, format, b));
        let sprite_buffer =
            create_instance_buffer::<SpriteInstance>(&device, INITIAL_SPRITE_CAPACITY);
        let fog = fog::FogPass::new(&device, HDR_FORMAT);
        let target = create_target(&device, view_scale(config.width, config.height));
        let post = Post::new(
            &device,
            &tex_layout,
            &sampler,
            format,
            &target.view,
            &target.material,
            target.size(),
        );

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
            material_pipeline,
            default_material,
            materials: 0,
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
            self.post.resize(
                &self.device,
                &self.sampler,
                &self.target.view,
                &self.target.material,
                self.target.size(),
            );
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
        self.parallelogram((x, y), (w, 0.0), (0.0, h), color);
    }

    /// Queues a solid line from `a` to `b` in world pixels, `width` thick
    /// (centred on the line), with flat ends. Like everything else it is
    /// shifted by the offset and scaled by the draw scale, so the width
    /// scales too: divide it by the zoom for a constant on-screen width.
    pub fn line(&mut self, a: (f32, f32), b: (f32, f32), width: f32, color: Color) {
        let (dx, dy) = (b.0 - a.0, b.1 - a.1);
        let len = dx.hypot(dy);
        if len <= f32::EPSILON || width <= 0.0 {
            return;
        }
        // The line's unit normal, scaled to half the width.
        let (nx, ny) = (-dy / len * width * 0.5, dx / len * width * 0.5);
        self.parallelogram((a.0 + nx, a.1 + ny), (dx, dy), (-2.0 * nx, -2.0 * ny), color);
    }

    /// Queues a solid parallelogram: `origin`, then the two edges leaving it.
    /// This is what [`Renderer::rect`] and [`Renderer::line`] draw, and what
    /// a trail's segments are made of.
    pub fn parallelogram(
        &mut self,
        origin: (f32, f32),
        edge_x: (f32, f32),
        edge_y: (f32, f32),
        color: Color,
    ) {
        let s = self.draw_scale;
        let pos = [(origin.0 + self.offset[0]) * s, (origin.1 + self.offset[1]) * s];
        self.rects.push(RectInstance {
            pos,
            edge_x: [edge_x.0 * s, edge_x.1 * s],
            edge_y: [edge_y.0 * s, edge_y.1 * s],
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
            // A material belongs to the old pixels: it stays only if the new
            // image is the same size, so its normals still line up.
            let had = slot.material.is_some();
            let material = slot.material.take().filter(|_| slot.size == texture.size);
            if had && material.is_none() {
                self.materials -= 1;
            }
            *slot = Texture { material, ..texture };
        }
    }

    /// Gives a texture a material: its normal map and light mask packed into
    /// one `width` x `height` image, as `reign_assets::lighting::compose_material`
    /// makes it (red and green are the normal's x and y, blue the mask, alpha
    /// 255 where the texture has maps). It must be the same size as the
    /// texture, so each texel lines up with its own. Lights shade the
    /// texture's sprites by it. Read as data, not as colour.
    pub fn set_material(&mut self, id: TextureId, rgba: &[u8], width: u32, height: u32) {
        assert_eq!(rgba.len(), (width * height * 4) as usize, "RGBA8 data size");
        let Some(size) = self.textures.get(id.0).map(|t| t.size) else { return };
        if size != (width, height) {
            log::warn!("material is {width}x{height}, its texture {}x{}: ignored", size.0, size.1);
            self.clear_material(id);
            return;
        }
        let bind = texture_bind(
            &self.device,
            &self.queue,
            &self.tex_layout,
            &self.sampler,
            rgba,
            (width, height),
            MATERIAL_FORMAT,
        );
        if let Some(slot) = self.textures.get_mut(id.0) {
            if slot.material.is_none() {
                self.materials += 1;
            }
            slot.material = Some(bind);
        }
    }

    /// Takes a texture's material away: its sprites light the plain way again.
    pub fn clear_material(&mut self, id: TextureId) {
        if let Some(slot) = self.textures.get_mut(id.0)
            && slot.material.take().is_some()
        {
            self.materials -= 1;
        }
    }

    /// A loaded texture's size in texels.
    pub fn texture_size(&self, id: TextureId) -> Option<(u32, u32)> {
        self.textures.get(id.0).map(|t| t.size)
    }

    fn upload(&mut self, rgba: &[u8], width: u32, height: u32) -> Texture {
        let bind_group = texture_bind(
            &self.device,
            &self.queue,
            &self.tex_layout,
            &self.sampler,
            rgba,
            (width, height),
            TEXTURE_FORMAT,
        );
        Texture { size: (width, height), bind_group, material: None }
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
        let light = Light {
            x: light.x * s,
            y: light.y * s,
            radius: light.radius * s,
            height: light.height * s,
            ..light
        };
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
        {
            // What each pixel of the world is made of, for the lights: cleared
            // to "no maps", then the sprites again where they have them.
            let none = Color { r: 0.0, g: 0.0, b: 0.0, a: 0.0 };
            let mut pass =
                begin_pass(&mut encoder, &self.target.material, Some(none), "material pass");
            if self.materials > 0 {
                self.draw_materials(&mut pass);
            }
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

    /// The world's sprites into the material target, in the order they were
    /// drawn, so the frontmost sprite at a pixel decides its material.
    /// Additive sprites (glows, flares) only add light and leave it alone;
    /// rectangles and fog are skipped.
    fn draw_materials(&self, pass: &mut wgpu::RenderPass) {
        pass.set_pipeline(&self.material_pipeline);
        pass.set_vertex_buffer(0, self.sprite_buffer.slice(..));
        for batch in &self.batches {
            if let Batch::Sprites { blend: Blend::Alpha, texture, start, end } = *batch {
                let t = &self.textures[texture];
                pass.set_bind_group(0, &t.bind_group, &[]);
                pass.set_bind_group(1, t.material.as_ref().unwrap_or(&self.default_material), &[]);
                pass.draw(0..6, start..end);
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
    let size = (VIEW_WIDTH * scale, VIEW_HEIGHT * scale);
    let view = post::hdr_target(device, "world target", size);
    let material = material_target(device, size);
    Target { scale, view, material }
}

/// Where the material pass draws: normals, masks and a has-maps flag.
fn material_target(device: &wgpu::Device, size: (u32, u32)) -> wgpu::TextureView {
    device
        .create_texture(&wgpu::TextureDescriptor {
            label: Some("material target"),
            size: wgpu::Extent3d { width: size.0, height: size.1, depth_or_array_layers: 1 },
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: MATERIAL_FORMAT,
            usage: wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::TEXTURE_BINDING,
            view_formats: &[],
        })
        .create_view(&wgpu::TextureViewDescriptor::default())
}

/// Uploads RGBA8 pixels as a texture and binds it with the pixel sampler.
fn texture_bind(
    device: &wgpu::Device,
    queue: &wgpu::Queue,
    layout: &wgpu::BindGroupLayout,
    sampler: &wgpu::Sampler,
    rgba: &[u8],
    (width, height): (u32, u32),
    format: wgpu::TextureFormat,
) -> wgpu::BindGroup {
    assert_eq!(rgba.len(), (width * height * 4) as usize, "RGBA8 data size");
    let size = wgpu::Extent3d { width, height, depth_or_array_layers: 1 };
    let texture = device.create_texture(&wgpu::TextureDescriptor {
        label: Some("sprite texture"),
        size,
        mip_level_count: 1,
        sample_count: 1,
        dimension: wgpu::TextureDimension::D2,
        format,
        usage: wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_DST,
        view_formats: &[],
    });
    queue.write_texture(
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
    bind_texture(device, layout, &view, sampler)
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
    const ATTRS: [wgpu::VertexAttribute; 4] =
        wgpu::vertex_attr_array![0 => Float32x2, 1 => Float32x2, 2 => Float32x2, 3 => Float32x4];
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

/// The sprite shader's vertex stage with a fragment stage that writes the
/// sprite's material instead of its colour: group 0 is the sprite's texture
/// (for its alpha), group 1 its material.
fn create_material_pipeline(
    device: &wgpu::Device,
    texture_layout: &wgpu::BindGroupLayout,
) -> wgpu::RenderPipeline {
    let shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
        label: Some("material shader"),
        source: wgpu::ShaderSource::Wgsl(shaders::SPRITE_MATERIAL.into()),
    });
    let layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
        label: Some("material layout"),
        bind_group_layouts: &[Some(texture_layout), Some(texture_layout)],
        ..Default::default()
    });
    const ATTRS: [wgpu::VertexAttribute; 6] = wgpu::vertex_attr_array![
        0 => Float32x2, 1 => Float32x2, 2 => Float32x2, 3 => Float32x2, 4 => Float32x4, 5 => Float32x2
    ];
    device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
        label: Some("material pipeline"),
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
                format: MATERIAL_FORMAT,
                // Replace: the frontmost sprite's material wins outright.
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
            // The material pass and the light pass that reads its result.
            let _ = create_material_pipeline(&device, &layout);
            let _ = post::light_pipeline(&device, &layout);
            assert!(scope.pop().await.is_none(), "sprite shader or instance layout is invalid");
        });
    }

    /// A device with no window, or `None` where there is no GPU to test on.
    fn gpu() -> Option<(wgpu::Device, wgpu::Queue)> {
        pollster::block_on(async {
            let instance = wgpu::Instance::new(
                wgpu::InstanceDescriptor::new_without_display_handle_from_env(),
            );
            let Ok(adapter) =
                instance.request_adapter(&wgpu::RequestAdapterOptions::default()).await
            else {
                eprintln!("No GPU adapter available: skipping a rendering test");
                return None;
            };
            adapter.request_device(&wgpu::DeviceDescriptor::default()).await.ok()
        })
    }

    /// Copies a `width` x `height` texture with `bytes` bytes a pixel into
    /// memory, rows packed.
    fn read_back(
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        texture: &wgpu::Texture,
        (width, height): (u32, u32),
        bytes: u32,
    ) -> Vec<u8> {
        let row = width * bytes;
        let padded = row.div_ceil(256) * 256;
        let buffer = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("readback"),
            size: u64::from(padded * height),
            usage: wgpu::BufferUsages::COPY_DST | wgpu::BufferUsages::MAP_READ,
            mapped_at_creation: false,
        });
        let mut encoder = device.create_command_encoder(&Default::default());
        encoder.copy_texture_to_buffer(
            wgpu::TexelCopyTextureInfo {
                texture,
                mip_level: 0,
                origin: wgpu::Origin3d::ZERO,
                aspect: wgpu::TextureAspect::All,
            },
            wgpu::TexelCopyBufferInfo {
                buffer: &buffer,
                layout: wgpu::TexelCopyBufferLayout {
                    offset: 0,
                    bytes_per_row: Some(padded),
                    rows_per_image: Some(height),
                },
            },
            wgpu::Extent3d { width, height, depth_or_array_layers: 1 },
        );
        queue.submit([encoder.finish()]);
        buffer.slice(..).map_async(wgpu::MapMode::Read, |result| result.unwrap());
        device.poll(wgpu::PollType::wait_indefinitely()).unwrap();
        let data = buffer.slice(..).get_mapped_range().expect("the buffer is mapped");
        let mut out = Vec::new();
        for y in 0..height {
            let start = (y * padded) as usize;
            out.extend_from_slice(&data[start..start + row as usize]);
        }
        out
    }

    fn half(bits: u16) -> f32 {
        let sign = if bits >> 15 == 1 { -1.0 } else { 1.0 };
        let (exp, man) = (i32::from((bits >> 10) & 0x1f), f32::from(bits & 0x3ff));
        sign * match exp {
            0 => man * 2f32.powi(-24),
            31 => f32::INFINITY,
            e => (1.0 + man / 1024.0) * 2f32.powi(e - 15),
        }
    }

    /// The light shader's formula, on the CPU: one light, one pixel's material.
    fn expected(at: (f32, f32), light: (f32, f32, f32, f32), material: [u8; 4]) -> f32 {
        let (cx, cy, radius, height) = light;
        let (lx, ly) = ((at.0 - cx) / radius, (at.1 - cy) / radius);
        let d2 = lx * lx + ly * ly;
        let edge = (1.0 - d2).clamp(0.0, 1.0);
        let falloff = edge * edge / (1.0 + 8.0 * d2);
        if material[3] < 128 {
            return falloff;
        }
        let nx = f32::from(material[0]) / 255.0 * 2.0 - 1.0;
        let ny = f32::from(material[1]) / 255.0 * 2.0 - 1.0;
        let nz = (1.0 - nx * nx - ny * ny).max(0.0).sqrt();
        let (tx, ty, tz) = (cx - at.0, -(cy - at.1), height);
        let len = (tx * tx + ty * ty + tz * tz).sqrt();
        let facing = ((nx * tx + ny * ty + nz * tz) / len).max(0.0);
        falloff * facing * f32::from(material[2]) / 255.0
    }

    #[test]
    fn lights_shade_pixels_by_their_normal_and_mask_and_leave_the_rest_alone() {
        let Some((device, queue)) = gpu() else { return };
        let (layout, sampler) = create_texture_binding(&device);
        // A 640x360 material: the top half has maps, the bottom half none.
        let (w, h) = (VIEW_WIDTH, VIEW_HEIGHT);
        let facing_right = [204u8, 128, 255, 255];
        let material_at = |x: u32, y: u32| -> [u8; 4] {
            if y >= 180 {
                [0, 0, 0, 0]
            } else if !(100..160).contains(&x) {
                facing_right
            } else if x < 140 {
                [204, 128, 0, 255]
            } else {
                [128, 128, 128, 255]
            }
        };
        let mut pixels = Vec::new();
        for y in 0..h {
            for x in 0..w {
                pixels.extend(material_at(x, y));
            }
        }
        let material =
            texture_bind(&device, &queue, &layout, &sampler, &pixels, (w, h), MATERIAL_FORMAT);
        let target = post::hdr_target_texture(&device, (w, h));
        let view = target.create_view(&Default::default());
        let pipeline = post::light_pipeline(&device, &layout);
        // One light over the top half, one over the bottom half.
        let lights = [(160.0f32, 90.0f32), (160.0, 270.0)];
        let mut bytes = Vec::new();
        for (x, y) in lights {
            bytes.extend(post::light_instance_bytes([x, y], 100.0, 24.0, [1.0, 1.0, 1.0, 1.0]));
        }
        let buffer = create_instance_buffer::<[f32; 8]>(&device, 2);
        queue.write_buffer(&buffer, 0, &bytes);
        let mut encoder = device.create_command_encoder(&Default::default());
        {
            let mut pass = begin_pass(&mut encoder, &view, Some(Color::BLACK), "test light pass");
            pass.set_pipeline(&pipeline);
            pass.set_bind_group(0, &material, &[]);
            pass.set_vertex_buffer(0, buffer.slice(..));
            pass.draw(0..6, 0..2);
        }
        queue.submit([encoder.finish()]);
        let out = read_back(&device, &queue, &target, (w, h), 8);
        let red = |x: u32, y: u32| {
            let i = ((y * w + x) * 8) as usize;
            half(u16::from_le_bytes([out[i], out[i + 1]]))
        };
        // (pixel, which light) pairs: under the light, toward it, away from
        // it, through a zero mask, through half a mask, and unmapped.
        let cases = [
            ((160, 90), 0),
            ((90, 90), 0),
            ((120, 60), 0),
            ((150, 100), 0),
            ((110, 90), 0),
            ((200, 90), 0),
            ((200, 270), 1),
            ((120, 250), 1),
        ];
        for ((x, y), which) in cases {
            let (lx, ly) = lights[which];
            let want = expected(
                (x as f32 + 0.5, y as f32 + 0.5),
                (lx, ly, 100.0, 24.0),
                material_at(x, y),
            );
            let got = red(x, y);
            assert!(
                (got - want).abs() < 0.01 + want * 0.02,
                "pixel ({x}, {y}): got {got}, want {want}"
            );
        }
        // The cases mean what they say.
        assert!(red(200, 90) < 0.001, "a surface facing away gets nothing");
        assert!(red(110, 90) < 0.001, "a zero mask gets nothing");
        assert!(red(90, 90) > 0.03, "a surface facing the light is lit: {}", red(90, 90));
        let flat_unmapped = red(200, 270);
        assert!(
            (flat_unmapped - 0.309).abs() < 0.01,
            "unmapped pixels get the plain disc: {flat_unmapped}"
        );
        assert!(red(150, 100) < red(160, 90), "half a mask is dimmer than the light's own spot");
    }

    #[test]
    fn the_material_pass_writes_the_frontmost_sprites_material_and_skips_clear_pixels() {
        let Some((device, queue)) = gpu() else { return };
        let (layout, sampler) = create_texture_binding(&device);
        let pipeline = create_material_pipeline(&device, &layout);
        let (w, h) = (VIEW_WIDTH, VIEW_HEIGHT);
        // Sprite A: 2x2 texels, the bottom-left one see-through, with a material.
        let a_pixels: Vec<u8> = vec![
            255, 0, 0, 255, 255, 0, 0, 255, //
            255, 0, 0, 0, 255, 0, 0, 255,
        ];
        let a_color =
            texture_bind(&device, &queue, &layout, &sampler, &a_pixels, (2, 2), TEXTURE_FORMAT);
        let a_material = texture_bind(
            &device,
            &queue,
            &layout,
            &sampler,
            &[200, 100, 50, 255].repeat(4),
            (2, 2),
            MATERIAL_FORMAT,
        );
        // Sprite B: opaque, with no material of its own.
        let b_color = texture_bind(
            &device,
            &queue,
            &layout,
            &sampler,
            &[0, 255, 0, 255].repeat(4),
            (2, 2),
            TEXTURE_FORMAT,
        );
        let none = texture_bind(
            &device,
            &queue,
            &layout,
            &sampler,
            &[0, 0, 0, 0],
            (1, 1),
            MATERIAL_FORMAT,
        );
        let sprite = |x: f32, y: f32| SpriteInstance {
            pos: [x, y],
            size: [4.0, 4.0],
            uv_min: [0.0, 0.0],
            uv_max: [1.0, 1.0],
            color: [1.0; 4],
            crimson: [0.0; 2],
        };
        let instances = [sprite(10.0, 10.0), sprite(12.0, 10.0)];
        let buffer = create_instance_buffer::<SpriteInstance>(&device, 2);
        queue.write_buffer(&buffer, 0, bytemuck::cast_slice(&instances));
        let target = device.create_texture(&wgpu::TextureDescriptor {
            label: Some("test material target"),
            size: wgpu::Extent3d { width: w, height: h, depth_or_array_layers: 1 },
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: MATERIAL_FORMAT,
            usage: wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::COPY_SRC,
            view_formats: &[],
        });
        let view = target.create_view(&Default::default());
        let mut encoder = device.create_command_encoder(&Default::default());
        {
            let clear = Color { r: 0.0, g: 0.0, b: 0.0, a: 0.0 };
            let mut pass = begin_pass(&mut encoder, &view, Some(clear), "test material pass");
            pass.set_pipeline(&pipeline);
            pass.set_vertex_buffer(0, buffer.slice(..));
            pass.set_bind_group(0, &a_color, &[]);
            pass.set_bind_group(1, &a_material, &[]);
            pass.draw(0..6, 0..1);
            pass.set_bind_group(0, &b_color, &[]);
            pass.set_bind_group(1, &none, &[]);
            pass.draw(0..6, 1..2);
        }
        queue.submit([encoder.finish()]);
        let out = read_back(&device, &queue, &target, (w, h), 4);
        let px = |x: u32, y: u32| {
            let i = ((y * w + x) * 4) as usize;
            [out[i], out[i + 1], out[i + 2], out[i + 3]]
        };
        assert_eq!(px(10, 10), [200, 100, 50, 255], "A's material where only A is");
        assert_eq!(px(11, 11), [200, 100, 50, 255]);
        assert_eq!(px(10, 12), [0, 0, 0, 0], "A's see-through texel writes nothing");
        assert_eq!(px(13, 10), [0, 0, 0, 0], "B drawn later wins: no maps, the old light");
        assert_eq!(px(15, 11), [0, 0, 0, 0]);
        assert_eq!(px(20, 20), [0, 0, 0, 0], "untouched pixels have none");
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
