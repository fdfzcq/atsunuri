//! Client-side window decorations (title bar, borders) for compositors such
//! as GNOME that leave drawing them to the app.

use std::num::NonZeroU32;
use std::sync::Arc;
use std::time::Duration;

use sctk_adwaita::{AdwaitaFrame, FrameConfig};
use smithay_client_toolkit::{
    compositor::CompositorState,
    reexports::csd_frame::{CursorIcon, DecorationsFrame, FrameAction, FrameClick, ResizeEdge},
    shell::xdg::{
        XdgSurface,
        window::{DecorationMode, Window, WindowConfigure},
    },
    shm::Shm,
    subcompositor::SubcompositorState,
};
use wayland_client::{QueueHandle, backend::ObjectId, protocol::wl_seat::WlSeat};
use wayland_protocols::xdg::shell::client::xdg_toplevel::ResizeEdge as XdgResizeEdge;

use crate::app::App;

pub struct Decorations {
    // Declared first so the frame's surfaces go before the globals.
    frame: Option<AdwaitaFrame<App>>,
    title: String,
    shm: Shm,
    compositor: Arc<CompositorState>,
    subcompositor: Arc<SubcompositorState>,
}

impl Decorations {
    pub fn new(
        title: &str,
        shm: Shm,
        compositor: Arc<CompositorState>,
        subcompositor: Arc<SubcompositorState>,
    ) -> Self {
        Decorations {
            frame: None,
            title: title.to_owned(),
            shm,
            compositor,
            subcompositor,
        }
    }

    pub fn shm(&mut self) -> &mut Shm {
        &mut self.shm
    }

    /// Applies a window configure and returns the size left for the content.
    pub fn configure(
        &mut self,
        window: &Window,
        qh: &QueueHandle<App>,
        configure: &WindowConfigure,
        current: (u32, u32),
    ) -> (u32, u32) {
        let fallback = (
            NonZeroU32::new(current.0).unwrap_or(NonZeroU32::MIN),
            NonZeroU32::new(current.1).unwrap_or(NonZeroU32::MIN),
        );

        if configure.decoration_mode != DecorationMode::Client {
            if let Some(frame) = &mut self.frame {
                frame.set_hidden(true);
            }
            let width = configure.new_size.0.unwrap_or(fallback.0).get();
            let height = configure.new_size.1.unwrap_or(fallback.1).get();
            window
                .xdg_surface()
                .set_window_geometry(0, 0, width as i32, height as i32);
            return (width, height);
        }

        if self.frame.is_none() {
            match AdwaitaFrame::new(
                window,
                &self.shm,
                self.compositor.clone(),
                self.subcompositor.clone(),
                qh.clone(),
                FrameConfig::auto(),
            ) {
                Ok(mut frame) => {
                    frame.set_title(self.title.clone());
                    self.frame = Some(frame);
                }
                Err(e) => {
                    log::warn!("failed to create window decorations: {e}");
                    let (width, height) = (fallback.0.get(), fallback.1.get());
                    window
                        .xdg_surface()
                        .set_window_geometry(0, 0, width as i32, height as i32);
                    return (width, height);
                }
            }
        }
        let frame = self.frame.as_mut().unwrap();
        frame.set_hidden(false);
        frame.update_state(configure.state);
        frame.update_wm_capabilities(configure.capabilities);

        // The configured size includes the title bar.
        let (width, height) = match configure.new_size {
            (Some(width), Some(height)) => {
                let (w, h) = frame.subtract_borders(width, height);
                (w.unwrap_or(NonZeroU32::MIN), h.unwrap_or(NonZeroU32::MIN))
            }
            _ => fallback,
        };
        frame.resize(width, height);

        let (x, y) = frame.location();
        let (outer_width, outer_height) = frame.add_borders(width.get(), height.get());
        window
            .xdg_surface()
            .set_window_geometry(x, y, outer_width as i32, outer_height as i32);
        (width.get(), height.get())
    }

    pub fn set_scale(&mut self, scale: u32) {
        if let Some(frame) = &mut self.frame {
            frame.set_scaling_factor(scale as f64);
        }
    }

    /// Redraws the decorations if needed. Returns true if the window surface
    /// must be committed for the change to show.
    pub fn draw(&mut self) -> bool {
        match &mut self.frame {
            Some(frame) if frame.is_dirty() && !frame.is_hidden() => {
                frame.draw();
                true
            }
            _ => false,
        }
    }

    /// Tracks a pointer or pen moving over `surface`. Returns the cursor to
    /// show if the surface is part of the decorations.
    pub fn moved(&mut self, time: u32, surface: &ObjectId, x: f64, y: f64) -> Option<CursorIcon> {
        let frame = self.frame.as_mut()?;
        frame.click_point_moved(Duration::from_millis(time as u64), surface, x, y)
    }

    pub fn left(&mut self) {
        if let Some(frame) = &mut self.frame {
            frame.click_point_left();
        }
    }

    pub fn click(&mut self, time: u32, click: FrameClick, pressed: bool) -> Option<FrameAction> {
        let frame = self.frame.as_mut()?;
        frame.on_click(Duration::from_millis(time as u64), click, pressed)
    }
}

/// Carries out a title bar or border action. `serial` must come from the
/// input event that triggered it.
pub fn apply_action(app: &mut App, seat: &WlSeat, serial: u32, action: FrameAction) {
    let window = &app.window;
    match action {
        FrameAction::Close => app.exit = true,
        FrameAction::Minimize => window.set_minimized(),
        FrameAction::Maximize => window.set_maximized(),
        FrameAction::UnMaximize => window.unset_maximized(),
        FrameAction::ShowMenu(x, y) => window.show_window_menu(seat, serial, (x, y)),
        FrameAction::Move => window.move_(seat, serial),
        FrameAction::Resize(edge) => {
            let edge = match edge {
                ResizeEdge::Top => XdgResizeEdge::Top,
                ResizeEdge::Bottom => XdgResizeEdge::Bottom,
                ResizeEdge::Left => XdgResizeEdge::Left,
                ResizeEdge::TopLeft => XdgResizeEdge::TopLeft,
                ResizeEdge::BottomLeft => XdgResizeEdge::BottomLeft,
                ResizeEdge::Right => XdgResizeEdge::Right,
                ResizeEdge::TopRight => XdgResizeEdge::TopRight,
                ResizeEdge::BottomRight => XdgResizeEdge::BottomRight,
                _ => return,
            };
            window.resize(seat, serial, edge);
        }
        _ => {}
    }
}
