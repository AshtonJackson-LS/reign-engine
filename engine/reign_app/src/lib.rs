//! The shell a Reign game runs in: the window and event loop, the fixed
//! simulation tick with render interpolation, gamepad polling, resizing
//! and the frame cap.
//!
//! A game implements [`App`] and hands it to [`run`]. Each frame the shell
//! polls the pads and calls, in order:
//!
//! 1. [`App::update`] once, with the frame's real time and pad activity;
//! 2. [`App::tick`] once per fixed tick due ([`reign_core::TICK_HZ`] by
//!    default), which is where the deterministic simulation steps;
//! 3. [`App::late_update`] once, after the ticks;
//! 4. [`App::draw`] with the interpolation alpha between the last tick and
//!    the next.
//!
//! Window events other than redraws go to [`App::event`] as they arrive.
//! Closing the window and resizing it are handled here first.
//!
//! Wall-clock time, frame rate and the window never reach the simulation:
//! ticks only happen in whole steps of the fixed clock.

pub mod pads;

use std::sync::Arc;
use std::time::{Duration, Instant};

pub use gilrs;
pub use winit;

use reign_assets::RgbaImage;
use reign_core::{FixedClock, TICK_HZ};
use reign_render::{Renderer, VIEW_HEIGHT, VIEW_WIDTH, integer_viewport};
use winit::application::ApplicationHandler;
use winit::dpi::PhysicalSize;
use winit::event::WindowEvent;
use winit::event_loop::{ActiveEventLoop, ControlFlow, EventLoop};
use winit::window::{Fullscreen, Icon, Window, WindowId};

pub use pads::{PadFrame, Pads};

/// How the window and loop start.
#[derive(Clone, Debug)]
pub struct AppConfig {
    pub title: String,
    /// Initial inner size, in physical pixels.
    pub size: (u32, u32),
    /// Smallest inner size, in physical pixels.
    pub min_size: (u32, u32),
    /// Start borderless fullscreen on the current monitor.
    pub fullscreen: bool,
    /// Title bar and taskbar icon. macOS ignores it.
    pub icon: Option<RgbaImage>,
    /// Fixed simulation ticks per second.
    pub tick_hz: u32,
    /// Quit by itself after this long, for unattended runs.
    pub exit_after: Option<Duration>,
}

impl Default for AppConfig {
    fn default() -> Self {
        AppConfig {
            title: "Reign".into(),
            size: (VIEW_WIDTH * 2, VIEW_HEIGHT * 2),
            min_size: (VIEW_WIDTH, VIEW_HEIGHT),
            fullscreen: false,
            icon: None,
            tick_hz: TICK_HZ,
            exit_after: None,
        }
    }
}

/// One frame's real time and pad activity, given to [`App::update`] and
/// [`App::late_update`]. Never feed `dt` into the simulation.
#[derive(Debug)]
pub struct Frame {
    /// Seconds since the last frame, capped at 0.1.
    pub dt: f32,
    /// Fixed ticks due this frame; [`App::tick`] runs this many times.
    pub ticks: u32,
    /// What the gamepads did since the last frame.
    pub pads: PadFrame,
}

/// A game, or anything else that runs in a Reign window.
pub trait App {
    /// Once per frame, before the ticks.
    fn update(&mut self, _ctx: &mut Ctx, _frame: &Frame) {}

    /// Once per fixed tick due. This is where the simulation steps.
    fn tick(&mut self, ctx: &mut TickCtx);

    /// Once per frame, after the ticks.
    fn late_update(&mut self, _ctx: &mut Ctx, _frame: &Frame) {}

    /// Draws and presents the frame. `alpha` is how far between the last
    /// tick and the next the frame falls, in 0..1, for interpolation.
    fn draw(&mut self, r: &mut Renderer, alpha: f32);

    /// A window event: input, focus and so on. Close and resize have
    /// already been handled; redraws never arrive here.
    fn event(&mut self, _ctx: &mut Ctx, _event: &WindowEvent) {}
}

/// The window, renderer and loop, outside the fixed tick.
pub struct Ctx<'a> {
    window: &'a Window,
    renderer: &'a mut Renderer,
    pads: &'a Pads,
    event_loop: &'a ActiveEventLoop,
    /// `None` when hosted: the host keeps its own frame pacing.
    frame_cap: Option<&'a mut u32>,
    /// Where the view sits in the window when a host draws it somewhere
    /// other than the whole window: x, y, width, height in physical pixels.
    view_rect: Option<[f32; 4]>,
    /// A host owns the window: fullscreen, size and frame cap requests are
    /// ignored.
    hosted: bool,
}

impl<'a> Ctx<'a> {
    /// A context for an app running inside a host, such as an editor, that
    /// owns the window and loop itself. The app's fullscreen, window size
    /// and frame cap requests are ignored; `view_rect` is where the host
    /// shows the view (x, y, width, height in physical window pixels), for
    /// [`Ctx::view_point`].
    pub fn hosted(
        window: &'a Window,
        renderer: &'a mut Renderer,
        pads: &'a Pads,
        event_loop: &'a ActiveEventLoop,
        view_rect: Option<[f32; 4]>,
    ) -> Self {
        Ctx { window, renderer, pads, event_loop, frame_cap: None, view_rect, hosted: true }
    }
}

impl Ctx<'_> {
    pub fn renderer(&mut self) -> &mut Renderer {
        self.renderer
    }

    pub fn window(&self) -> &Window {
        self.window
    }

    pub fn pads(&self) -> &Pads {
        self.pads
    }

    /// Ends the event loop once the current callback returns.
    pub fn exit(&self) {
        self.event_loop.exit();
    }

    pub fn is_fullscreen(&self) -> bool {
        self.window.fullscreen().is_some()
    }

    /// Whether a host such as an editor owns the window.
    pub fn is_hosted(&self) -> bool {
        self.hosted
    }

    /// Borderless fullscreen on the current monitor, or windowed.
    pub fn set_fullscreen(&self, on: bool) {
        if !self.hosted && on != self.is_fullscreen() {
            self.window.set_fullscreen(on.then_some(Fullscreen::Borderless(None)));
        }
    }

    /// Asks for a new inner window size in physical pixels.
    pub fn request_size(&self, width: u32, height: u32) {
        if self.hosted {
            return;
        }
        let _ = self.window.request_inner_size(PhysicalSize::new(width, height));
    }

    pub fn set_vsync(&mut self, on: bool) {
        self.renderer.set_vsync(on);
    }

    /// Caps frames per second; 0 is uncapped.
    pub fn set_frame_cap(&mut self, fps: u32) {
        if let Some(cap) = &mut self.frame_cap {
            **cap = fps;
        }
    }

    /// A window position (as in cursor events) in view pixels, if it's over
    /// the view.
    pub fn view_point(&self, x: f64, y: f64) -> Option<(f32, f32)> {
        let [vx, vy, vw, vh] = self.view_rect.unwrap_or_else(|| {
            let size = self.window.inner_size();
            let (x, y, w, h) = integer_viewport(size.width, size.height);
            [x as f32, y as f32, w as f32, h as f32]
        });
        let px = (x as f32 - vx) / vw * VIEW_WIDTH as f32;
        let py = (y as f32 - vy) / vh * VIEW_HEIGHT as f32;
        let inside =
            (0.0..VIEW_WIDTH as f32).contains(&px) && (0.0..VIEW_HEIGHT as f32).contains(&py);
        inside.then_some((px, py))
    }
}

/// What a fixed tick may see: held input, not the window or renderer.
pub struct TickCtx<'a> {
    pads: &'a Pads,
    event_loop: &'a ActiveEventLoop,
    index: u32,
    count: u32,
}

impl<'a> TickCtx<'a> {
    /// A tick context for a host that runs the ticks itself: tick `index`
    /// of `count` this frame.
    pub fn hosted(pads: &'a Pads, event_loop: &'a ActiveEventLoop, index: u32, count: u32) -> Self {
        TickCtx { pads, event_loop, index, count }
    }
}

impl TickCtx<'_> {
    /// Gamepad state, to read held buttons for this tick's input.
    pub fn pads(&self) -> &Pads {
        self.pads
    }

    /// Which of this frame's ticks this is, from 0.
    pub fn index(&self) -> u32 {
        self.index
    }

    /// How many ticks this frame runs.
    pub fn count(&self) -> u32 {
        self.count
    }

    /// Ends the event loop once the frame is done.
    pub fn exit(&self) {
        self.event_loop.exit();
    }
}

/// Opens the window and runs `A` until it exits or the window closes.
/// `make` builds the app once the window and renderer exist, so it can load
/// textures and apply window settings.
pub fn run<A, F>(config: AppConfig, make: F)
where
    A: App,
    F: FnOnce(&mut Ctx) -> A,
{
    let event_loop = EventLoop::new().expect("create event loop");
    event_loop.set_control_flow(ControlFlow::Poll);
    let mut shell = Shell { config, make: Some(make), running: None };
    event_loop.run_app(&mut shell).expect("run event loop");
}

/// Opens a window as `config` describes and a renderer for it. [`run`]
/// uses it; hosts with their own loop, such as an editor, can too.
pub fn open_window(event_loop: &ActiveEventLoop, config: &AppConfig) -> (Arc<Window>, Renderer) {
    let c = config;
    let mut attrs = Window::default_attributes()
        .with_title(c.title.clone())
        .with_inner_size(PhysicalSize::new(c.size.0, c.size.1))
        .with_min_inner_size(PhysicalSize::new(c.min_size.0, c.min_size.1))
        .with_fullscreen(c.fullscreen.then_some(Fullscreen::Borderless(None)));
    if let Some(icon) = &c.icon {
        let icon = Icon::from_rgba(icon.rgba.clone(), icon.width, icon.height)
            .expect("window icon is valid RGBA");
        attrs = attrs.with_window_icon(Some(icon));
    }
    let window = Arc::new(event_loop.create_window(attrs).expect("create window"));
    let size = window.inner_size();
    let renderer = pollster::block_on(Renderer::new(window.clone(), (size.width, size.height)))
        .expect("initialise renderer");
    (window, renderer)
}

struct Shell<A, F> {
    config: AppConfig,
    make: Option<F>,
    running: Option<Running<A>>,
}

struct Running<A> {
    app: A,
    window: Arc<Window>,
    renderer: Renderer,
    pads: Pads,
    clock: FixedClock,
    started: Instant,
    last_frame: Instant,
    frame_cap: u32,
}

impl<A: App> Running<A> {
    fn ctx<'a>(&'a mut self, event_loop: &'a ActiveEventLoop) -> (&'a mut A, Ctx<'a>) {
        let ctx = Ctx {
            window: &self.window,
            renderer: &mut self.renderer,
            pads: &self.pads,
            event_loop,
            frame_cap: Some(&mut self.frame_cap),
            view_rect: None,
            hosted: false,
        };
        (&mut self.app, ctx)
    }

    fn frame(&mut self, event_loop: &ActiveEventLoop) {
        let now = Instant::now();
        let elapsed = now - self.last_frame;
        let dt = elapsed.as_secs_f32().min(0.1);
        let ticks = self.clock.advance(elapsed);
        self.last_frame = now;
        let frame = Frame { dt, ticks, pads: self.pads.poll(dt) };

        let (app, mut ctx) = self.ctx(event_loop);
        app.update(&mut ctx, &frame);
        for index in 0..ticks {
            let mut tick = TickCtx { pads: &self.pads, event_loop, index, count: ticks };
            self.app.tick(&mut tick);
        }
        let (app, mut ctx) = self.ctx(event_loop);
        app.late_update(&mut ctx, &frame);
        self.app.draw(&mut self.renderer, self.clock.alpha());
    }

    /// When the next frame is due under the frame cap, if it isn't yet.
    fn frame_wait(&self) -> Option<Instant> {
        if self.frame_cap == 0 {
            return None;
        }
        let due = self.last_frame + Duration::from_secs_f64(1.0 / self.frame_cap as f64);
        (Instant::now() < due).then_some(due)
    }
}

impl<A: App, F: FnOnce(&mut Ctx) -> A> ApplicationHandler for Shell<A, F> {
    fn resumed(&mut self, event_loop: &ActiveEventLoop) {
        let Some(make) = self.make.take() else { return };
        let (window, mut renderer) = open_window(event_loop, &self.config);
        let pads = Pads::new();
        let mut frame_cap = 0;
        let app = make(&mut Ctx {
            window: &window,
            renderer: &mut renderer,
            pads: &pads,
            event_loop,
            frame_cap: Some(&mut frame_cap),
            view_rect: None,
            hosted: false,
        });
        let now = Instant::now();
        self.running = Some(Running {
            app,
            window,
            renderer,
            pads,
            clock: FixedClock::new(self.config.tick_hz),
            started: now,
            last_frame: now,
            frame_cap,
        });
    }

    fn window_event(&mut self, event_loop: &ActiveEventLoop, _id: WindowId, event: WindowEvent) {
        let Some(running) = self.running.as_mut() else { return };
        match event {
            WindowEvent::RedrawRequested => return running.frame(event_loop),
            WindowEvent::CloseRequested => event_loop.exit(),
            WindowEvent::Resized(size) => running.renderer.resize(size.width, size.height),
            _ => {}
        }
        let (app, mut ctx) = running.ctx(event_loop);
        app.event(&mut ctx, &event);
    }

    fn about_to_wait(&mut self, event_loop: &ActiveEventLoop) {
        let Some(running) = &self.running else { return };
        if self.config.exit_after.is_some_and(|limit| running.started.elapsed() >= limit) {
            event_loop.exit();
            return;
        }
        match running.frame_wait() {
            Some(due) => event_loop.set_control_flow(ControlFlow::WaitUntil(due)),
            None => {
                event_loop.set_control_flow(ControlFlow::Poll);
                running.window.request_redraw();
            }
        }
    }
}
