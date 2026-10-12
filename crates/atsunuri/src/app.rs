use std::collections::HashSet;

use atsunuri_core::{Brush, Canvas, InputPoint, Stroke};
use smithay_client_toolkit::{
    compositor::{CompositorHandler, FrameCallbackData},
    dispatch2::Dispatch2,
    output::{OutputHandler, OutputState},
    registry::{ProvidesRegistryState, RegistryState},
    registry_handlers,
    seat::{
        Capability, SeatHandler, SeatState,
        pointer::{PointerEvent, PointerEventKind, PointerHandler},
    },
    shell::{
        WaylandSurface,
        xdg::window::{Window, WindowConfigure, WindowHandler},
    },
};
use wayland_client::{
    Connection, Proxy, QueueHandle, WEnum,
    backend::ObjectId,
    protocol::{wl_keyboard, wl_output, wl_pointer, wl_seat, wl_surface},
};
use wayland_protocols::wp::tablet::zv2::client::zwp_tablet_manager_v2::ZwpTabletManagerV2;

use crate::renderer::Renderer;
use crate::tablet::{SeatData, ToolFrame, ToolKind};
use crate::view::View;

/// Linux evdev key and button codes.
const KEY_ESC: u32 = 1;
const KEY_C: u32 = 46;
const BTN_LEFT: u32 = 0x110;

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
    pub renderer: Renderer,
    pub window: Window,
    pub registry_state: RegistryState,
    pub seat_state: SeatState,
    pub output_state: OutputState,
    pub tablet_manager: Option<ZwpTabletManagerV2>,
    tablet_seats: HashSet<ObjectId>,

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
    pub fn new(
        renderer: Renderer,
        window: Window,
        registry_state: RegistryState,
        seat_state: SeatState,
        output_state: OutputState,
        tablet_manager: Option<ZwpTabletManagerV2>,
        canvas: Canvas,
    ) -> Self {
        App {
            renderer,
            window,
            registry_state,
            seat_state,
            output_state,
            tablet_manager,
            tablet_seats: HashSet::new(),
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
        }
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
    pub fn draw_if_needed(&mut self, qh: &QueueHandle<Self>) {
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
        let point = self.to_canvas(frame.position, frame.pressure);
        // Only the pen tip paints for now; the eraser end is ignored.
        if frame.down && frame.kind == ToolKind::Pen {
            self.begin_stroke(Source::Tablet, point);
        } else if frame.moved {
            self.continue_stroke(Source::Tablet, point);
        }
        if frame.up {
            self.end_stroke(Source::Tablet);
        }
        if frame.proximity_out {
            self.tools_in_proximity = self.tools_in_proximity.saturating_sub(1);
            self.end_stroke(Source::Tablet);
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
        self.scale = new_factor.max(1) as u32;
        surface.set_buffer_scale(self.scale as i32);
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
        _: &QueueHandle<Self>,
        _: &Window,
        configure: WindowConfigure,
        _: u32,
    ) {
        let (width, height) = configure.new_size;
        self.size = (
            width.map_or(DEFAULT_WINDOW_SIZE.0, |w| w.get()),
            height.map_or(DEFAULT_WINDOW_SIZE.1, |h| h.get()),
        );
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
            Capability::Pointer => {
                if let Err(e) = self.seat_state.get_pointer(qh, &seat) {
                    log::warn!("failed to get pointer: {e}");
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
        _: &wl_pointer::WlPointer,
        events: &[PointerEvent],
    ) {
        for event in events {
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
