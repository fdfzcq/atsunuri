use std::collections::HashSet;
use std::ffi::c_void;
use std::ptr::NonNull;
use std::sync::Arc;

use atsunuri_core::{Brush, Canvas, InputPoint, Stroke};
use smithay_client_toolkit::{
    compositor::{CompositorHandler, CompositorState, FrameCallbackData},
    dispatch2::Dispatch2,
    output::{OutputHandler, OutputState},
    reexports::csd_frame::{CursorIcon, FrameClick},
    registry::{ProvidesRegistryState, RegistryState},
    registry_handlers,
    seat::{
        Capability, SeatHandler, SeatState,
        pointer::{
            PointerData, PointerEvent, PointerEventKind, PointerHandler, ThemeSpec, ThemedPointer,
        },
    },
    shell::{
        WaylandSurface,
        xdg::{
            XdgShell,
            window::{Window, WindowConfigure, WindowDecorations, WindowHandler},
        },
    },
    shm::{Shm, ShmHandler},
    subcompositor::SubcompositorState,
};
use wayland_client::{
    Connection, Proxy, QueueHandle, WEnum,
    backend::ObjectId,
    globals::GlobalList,
    protocol::{wl_keyboard, wl_output, wl_pointer, wl_seat, wl_surface},
};
use wayland_protocols::wp::tablet::zv2::client::zwp_tablet_manager_v2::ZwpTabletManagerV2;

use crate::decorations::{self, Decorations};
use crate::renderer::Renderer;
use crate::tablet::{ManagerData, SeatData, ToolFrame, ToolKind};
use crate::view::View;

const TITLE: &str = "Atsunuri";

/// Linux evdev key and button codes.
const KEY_ESC: u32 = 1;
const KEY_C: u32 = 46;
const BTN_LEFT: u32 = 0x110;
const BTN_RIGHT: u32 = 0x111;

/// Window size in logical pixels when the compositor leaves it to us.
const DEFAULT_WINDOW_SIZE: (u32, u32) = (1280, 800);

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Source {
    Mouse,
    Tablet,
}

pub struct App {
    // The renderer holds raw pointers to the window's surface, so it is
    // declared (and therefore dropped) before the window.
    renderer: Renderer,
    decorations: Decorations,
    pub window: Window,
    compositor: Arc<CompositorState>,
    registry_state: RegistryState,
    pub seat_state: SeatState,
    output_state: OutputState,
    tablet_manager: Option<ZwpTabletManagerV2>,
    tablet_seats: HashSet<ObjectId>,
    pointer: Option<ThemedPointer>,
    /// Cursor to show for the mouse, if it needs updating.
    pending_cursor: Option<CursorIcon>,

    canvas: Canvas,
    brush: Brush,
    stroke: Option<(Source, Stroke)>,
    /// Pen tools hovering over the window. Mouse input is ignored meanwhile.
    tools_in_proximity: u32,

    /// Window size in logical pixels.
    size: (u32, u32),
    scale: u32,
    view: View,
    configured: bool,
    needs_redraw: bool,
    frame_pending: bool,
    pub exit: bool,
}

impl App {
    /// Binds the Wayland globals and opens the window.
    pub fn new(
        conn: &Connection,
        globals: &GlobalList,
        qh: &QueueHandle<Self>,
        canvas: Canvas,
    ) -> Result<Self, String> {
        let compositor =
            CompositorState::bind(globals, qh).map_err(|e| format!("wl_compositor: {e}"))?;
        let subcompositor =
            SubcompositorState::bind(compositor.wl_compositor().clone(), globals, qh)
                .map_err(|e| format!("wl_subcompositor: {e}"))?;
        let shm = Shm::bind(globals, qh).map_err(|e| format!("wl_shm: {e}"))?;
        let xdg_shell = XdgShell::bind(globals, qh).map_err(|e| format!("xdg_shell: {e}"))?;
        let tablet_manager = globals.bind(qh, 1..=1, ManagerData).ok();
        if tablet_manager.is_none() {
            log::warn!("compositor has no tablet support; only the mouse will work");
        }

        let window = xdg_shell.create_window(
            compositor.create_surface(qh),
            WindowDecorations::ServerDefault,
            qh,
        );
        window.set_title(TITLE);
        window.set_app_id("io.github.fdfzcq.Atsunuri");
        window.set_min_size(Some((320, 240)));
        window.commit();

        let display = NonNull::new(conn.backend().display_ptr() as *mut c_void)
            .ok_or("Wayland display pointer is null")?;
        let surface = NonNull::new(window.wl_surface().id().as_ptr() as *mut c_void)
            .ok_or("Wayland surface pointer is null")?;
        // SAFETY: `App` drops the renderer before the window, and the caller
        // keeps the connection open for as long as the app exists.
        let renderer =
            unsafe { Renderer::new(display, surface, (canvas.width(), canvas.height()))? };

        let compositor = Arc::new(compositor);
        let decorations = Decorations::new(TITLE, shm, compositor.clone(), Arc::new(subcompositor));

        let mut app = App {
            renderer,
            decorations,
            window,
            compositor,
            registry_state: RegistryState::new(globals),
            seat_state: SeatState::new(globals, qh),
            output_state: OutputState::new(globals, qh),
            tablet_manager,
            tablet_seats: HashSet::new(),
            pointer: None,
            pending_cursor: None,
            canvas,
            brush: Brush::default(),
            stroke: None,
            tools_in_proximity: 0,
            size: DEFAULT_WINDOW_SIZE,
            scale: 1,
            view: View {
                offset: [0.0, 0.0],
                scale: 1.0,
            },
            configured: false,
            needs_redraw: true,
            frame_pending: false,
            exit: false,
        };
        for seat in app.seat_state.seats() {
            app.add_tablet_seat(qh, &seat);
        }
        Ok(app)
    }

    /// Listens for tablets on `seat`, once per seat.
    pub fn add_tablet_seat(&mut self, qh: &QueueHandle<Self>, seat: &wl_seat::WlSeat) {
        let Some(manager) = &self.tablet_manager else {
            return;
        };
        if self.tablet_seats.insert(seat.id()) {
            manager.get_tablet_seat(seat, qh, SeatData);
        }
    }

    /// Uploads canvas changes and draws a frame if one is due.
    pub fn draw_if_needed(&mut self, conn: &Connection, qh: &QueueHandle<Self>) {
        if let (Some(pointer), Some(icon)) = (&self.pointer, self.pending_cursor.take())
            && let Err(e) = pointer.set_cursor(conn, icon)
        {
            log::warn!("failed to set cursor: {e}");
        }
        if self.decorations.draw() {
            self.needs_redraw = true;
        }
        if let Some(dirty) = self.canvas.take_dirty() {
            self.renderer.upload(&self.canvas, dirty);
            self.needs_redraw = true;
        }
        if !self.configured || !self.needs_redraw || self.frame_pending {
            return;
        }
        let surface = self.window.wl_surface();
        // Ask to be told when the compositor wants the next frame. Requested
        // before presenting, since presenting commits the surface.
        surface.frame(qh, FrameCallbackData(surface.clone()));
        if self.renderer.render(&self.view) {
            self.needs_redraw = false;
            self.frame_pending = true;
        }
    }

    fn update_size(&mut self) {
        let physical = (self.size.0 * self.scale, self.size.1 * self.scale);
        if self.renderer.size() != physical {
            self.renderer.resize(physical.0, physical.1);
        }
        self.view = View::fit((self.canvas.width(), self.canvas.height()), physical);
        self.needs_redraw = true;
    }

    /// Converts a surface-local position in logical pixels to canvas pixels.
    fn to_canvas(&self, position: (f64, f64), pressure: f64) -> InputPoint {
        let scale = self.scale as f32;
        let (x, y) = self
            .view
            .window_to_canvas(position.0 as f32 * scale, position.1 as f32 * scale);
        InputPoint {
            x,
            y,
            pressure: pressure as f32,
        }
    }

    fn begin_stroke(&mut self, source: Source, point: InputPoint) {
        let stroke = Stroke::begin(&mut self.canvas, self.brush.clone(), point);
        self.stroke = Some((source, stroke));
    }

    fn continue_stroke(&mut self, source: Source, point: InputPoint) {
        if let Some((s, stroke)) = &mut self.stroke
            && *s == source
        {
            stroke.add(&mut self.canvas, point);
        }
    }

    fn end_stroke(&mut self, source: Source) {
        if matches!(self.stroke, Some((s, _)) if s == source) {
            self.stroke = None;
        }
    }

    pub fn tool_frame(&mut self, frame: &ToolFrame) {
        if frame.proximity_in {
            self.tools_in_proximity += 1;
        }
        if frame.proximity_out {
            self.tools_in_proximity = self.tools_in_proximity.saturating_sub(1);
        }
        match &frame.surface {
            Some(surface) if surface == self.window.wl_surface() => self.pen_on_canvas(frame),
            Some(surface) => self.pen_on_decorations(surface.id(), frame),
            None => {}
        }
    }

    fn pen_on_canvas(&mut self, frame: &ToolFrame) {
        let point = self.to_canvas(frame.position, frame.pressure);
        // Only the pen tip paints for now; the eraser end is ignored.
        if frame.down && frame.kind == ToolKind::Pen {
            self.begin_stroke(Source::Tablet, point);
        } else if frame.moved {
            self.continue_stroke(Source::Tablet, point);
        }
        if frame.up || frame.proximity_out {
            self.end_stroke(Source::Tablet);
        }
    }

    /// Lets the pen use the title bar and window borders like a mouse.
    fn pen_on_decorations(&mut self, surface: ObjectId, frame: &ToolFrame) {
        let (x, y) = frame.position;
        if frame.moved || frame.proximity_in {
            self.decorations.moved(frame.time, &surface, x, y);
        }
        for (happened, pressed) in [(frame.down, true), (frame.up, false)] {
            if !happened {
                continue;
            }
            let action = self
                .decorations
                .click(frame.time, FrameClick::Normal, pressed);
            if let (Some(action), Some(seat)) = (action, self.seat_state.seats().next()) {
                decorations::apply_action(self, &seat, frame.down_serial, action);
            }
        }
        if frame.proximity_out {
            self.decorations.left();
        }
    }

    fn key_pressed(&mut self, key: u32) {
        match key {
            KEY_ESC => self.exit = true,
            KEY_C => {
                self.stroke = None;
                self.canvas.clear();
            }
            _ => {}
        }
    }
}

impl CompositorHandler for App {
    fn scale_factor_changed(
        &mut self,
        _: &Connection,
        _: &QueueHandle<Self>,
        surface: &wl_surface::WlSurface,
        new_factor: i32,
    ) {
        // The decorations' surfaces get this too; they handle scaling themselves.
        if surface != self.window.wl_surface() {
            return;
        }
        self.scale = new_factor.max(1) as u32;
        surface.set_buffer_scale(self.scale as i32);
        self.decorations.set_scale(self.scale);
        self.update_size();
    }

    fn transform_changed(
        &mut self,
        _: &Connection,
        _: &QueueHandle<Self>,
        _: &wl_surface::WlSurface,
        _: wl_output::Transform,
    ) {
    }

    fn frame(&mut self, _: &Connection, _: &QueueHandle<Self>, _: &wl_surface::WlSurface, _: u32) {
        self.frame_pending = false;
    }

    fn surface_enter(
        &mut self,
        _: &Connection,
        _: &QueueHandle<Self>,
        _: &wl_surface::WlSurface,
        _: &wl_output::WlOutput,
    ) {
    }

    fn surface_leave(
        &mut self,
        _: &Connection,
        _: &QueueHandle<Self>,
        _: &wl_surface::WlSurface,
        _: &wl_output::WlOutput,
    ) {
    }
}

impl OutputHandler for App {
    fn output_state(&mut self) -> &mut OutputState {
        &mut self.output_state
    }

    fn new_output(&mut self, _: &Connection, _: &QueueHandle<Self>, _: wl_output::WlOutput) {}

    fn update_output(&mut self, _: &Connection, _: &QueueHandle<Self>, _: wl_output::WlOutput) {}

    fn output_destroyed(&mut self, _: &Connection, _: &QueueHandle<Self>, _: wl_output::WlOutput) {}
}

impl WindowHandler for App {
    fn request_close(&mut self, _: &Connection, _: &QueueHandle<Self>, _: &Window) {
        self.exit = true;
    }

    fn configure(
        &mut self,
        _: &Connection,
        qh: &QueueHandle<Self>,
        window: &Window,
        configure: WindowConfigure,
        _: u32,
    ) {
        self.size = self
            .decorations
            .configure(window, qh, &configure, self.size);
        self.configured = true;
        // A configure must be answered with a new frame.
        self.frame_pending = false;
        self.update_size();
    }
}

impl SeatHandler for App {
    fn seat_state(&mut self) -> &mut SeatState {
        &mut self.seat_state
    }

    fn new_seat(&mut self, _: &Connection, qh: &QueueHandle<Self>, seat: wl_seat::WlSeat) {
        self.add_tablet_seat(qh, &seat);
    }

    fn new_capability(
        &mut self,
        _: &Connection,
        qh: &QueueHandle<Self>,
        seat: wl_seat::WlSeat,
        capability: Capability,
    ) {
        match capability {
            Capability::Pointer if self.pointer.is_none() => {
                let cursor_surface = self.compositor.create_surface(qh);
                match self.seat_state.get_pointer_with_theme::<_, ()>(
                    qh,
                    &seat,
                    self.decorations.shm().wl_shm(),
                    cursor_surface,
                    ThemeSpec::default(),
                ) {
                    Ok(pointer) => self.pointer = Some(pointer),
                    Err(e) => log::warn!("failed to get pointer: {e}"),
                }
            }
            Capability::Keyboard => {
                seat.get_keyboard(qh, KeyboardData);
            }
            _ => {}
        }
    }

    fn remove_capability(
        &mut self,
        _: &Connection,
        _: &QueueHandle<Self>,
        _: wl_seat::WlSeat,
        _: Capability,
    ) {
    }

    fn remove_seat(&mut self, _: &Connection, _: &QueueHandle<Self>, seat: wl_seat::WlSeat) {
        self.tablet_seats.remove(&seat.id());
    }
}

impl PointerHandler for App {
    fn pointer_frame(
        &mut self,
        _: &Connection,
        _: &QueueHandle<Self>,
        pointer: &wl_pointer::WlPointer,
        events: &[PointerEvent],
    ) {
        for event in events {
            if &event.surface != self.window.wl_surface() {
                self.pointer_on_decorations(pointer, event);
                continue;
            }
            if let PointerEventKind::Enter { .. } = event.kind {
                self.pending_cursor = Some(CursorIcon::Crosshair);
            }
            if self.tools_in_proximity > 0 {
                continue;
            }
            let point = self.to_canvas(event.position, 1.0);
            match event.kind {
                PointerEventKind::Press {
                    button: BTN_LEFT, ..
                } => self.begin_stroke(Source::Mouse, point),
                PointerEventKind::Motion { .. } => self.continue_stroke(Source::Mouse, point),
                PointerEventKind::Release {
                    button: BTN_LEFT, ..
                }
                | PointerEventKind::Leave { .. } => self.end_stroke(Source::Mouse),
                _ => {}
            }
        }
    }
}

impl App {
    fn pointer_on_decorations(&mut self, pointer: &wl_pointer::WlPointer, event: &PointerEvent) {
        let (x, y) = event.position;
        let surface = event.surface.id();
        match event.kind {
            PointerEventKind::Enter { .. } => {
                self.pending_cursor = self.decorations.moved(0, &surface, x, y);
            }
            PointerEventKind::Motion { time } => {
                if let Some(icon) = self.decorations.moved(time, &surface, x, y) {
                    self.pending_cursor = Some(icon);
                }
            }
            PointerEventKind::Leave { .. } => self.decorations.left(),
            PointerEventKind::Press {
                button,
                serial,
                time,
            }
            | PointerEventKind::Release {
                button,
                serial,
                time,
            } => {
                let click = match button {
                    BTN_LEFT => FrameClick::Normal,
                    BTN_RIGHT => FrameClick::Alternate,
                    _ => return,
                };
                let pressed = matches!(event.kind, PointerEventKind::Press { .. });
                let action = self.decorations.click(time, click, pressed);
                let seat = pointer.data::<PointerData<()>>().map(|d| d.seat().clone());
                if let (Some(action), Some(seat)) = (action, seat) {
                    decorations::apply_action(self, &seat, serial, action);
                }
            }
            _ => {}
        }
    }
}

impl ShmHandler for App {
    fn shm_state(&mut self) -> &mut Shm {
        self.decorations.shm()
    }
}

/// Raw keyboard handling by evdev keycode. Enough for a couple of fixed
/// shortcuts without pulling in xkbcommon.
pub struct KeyboardData;

impl Dispatch2<wl_keyboard::WlKeyboard, App> for KeyboardData {
    fn event(
        &self,
        app: &mut App,
        _: &wl_keyboard::WlKeyboard,
        event: wl_keyboard::Event,
        _: &Connection,
        _: &QueueHandle<App>,
    ) {
        if let wl_keyboard::Event::Key {
            key,
            state: WEnum::Value(wl_keyboard::KeyState::Pressed),
            ..
        } = event
        {
            app.key_pressed(key);
        }
    }
}

smithay_client_toolkit::delegate_registry!(App);

impl ProvidesRegistryState for App {
    fn registry(&mut self) -> &mut RegistryState {
        &mut self.registry_state
    }
    registry_handlers![OutputState, SeatState];
}

smithay_client_toolkit::delegate_dispatch2!(App);
