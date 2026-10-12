mod app;
mod renderer;
mod tablet;
mod view;

use std::ffi::c_void;
use std::ptr::NonNull;

use atsunuri_core::Canvas;
use smithay_client_toolkit::{
    compositor::CompositorState,
    output::OutputState,
    registry::RegistryState,
    seat::SeatState,
    shell::{
        WaylandSurface,
        xdg::{XdgShell, window::WindowDecorations},
    },
};
use wayland_client::{Connection, Proxy, globals::registry_queue_init};

use crate::app::App;
use crate::renderer::Renderer;
use crate::tablet::ManagerData;

const CANVAS_SIZE: (u32, u32) = (1920, 1080);

fn main() {
    env_logger::Builder::from_env(env_logger::Env::default().default_filter_or("info")).init();
    if let Err(e) = run() {
        log::error!("{e}");
        std::process::exit(1);
    }
}

fn run() -> Result<(), String> {
    let conn = Connection::connect_to_env()
        .map_err(|e| format!("could not connect to a Wayland compositor: {e}"))?;
    let (globals, mut event_queue) =
        registry_queue_init::<App>(&conn).map_err(|e| format!("Wayland registry: {e}"))?;
    let qh = event_queue.handle();

    let compositor =
        CompositorState::bind(&globals, &qh).map_err(|e| format!("wl_compositor: {e}"))?;
    let xdg_shell = XdgShell::bind(&globals, &qh).map_err(|e| format!("xdg_shell: {e}"))?;
    let tablet_manager = globals.bind(&qh, 1..=1, ManagerData).ok();
    if tablet_manager.is_none() {
        log::warn!("compositor has no tablet support; only the mouse will work");
    }

    let window = xdg_shell.create_window(
        compositor.create_surface(&qh),
        WindowDecorations::ServerDefault,
        &qh,
    );
    window.set_title("Atsunuri");
    window.set_app_id("io.github.fdfzcq.Atsunuri");
    window.set_min_size(Some((320, 240)));
    window.commit();

    let display = NonNull::new(conn.backend().display_ptr() as *mut c_void)
        .ok_or("Wayland display pointer is null")?;
    let surface = NonNull::new(window.wl_surface().id().as_ptr() as *mut c_void)
        .ok_or("Wayland surface pointer is null")?;
    // SAFETY: `App` owns the window and drops the renderer first, and the
    // connection lives until the end of this function.
    let renderer = unsafe { Renderer::new(display, surface, CANVAS_SIZE)? };

    let mut app = App::new(
        renderer,
        window,
        RegistryState::new(&globals),
        SeatState::new(&globals, &qh),
        OutputState::new(&globals, &qh),
        tablet_manager,
        Canvas::new(CANVAS_SIZE.0, CANVAS_SIZE.1),
    );
    for seat in app.seat_state.seats() {
        app.add_tablet_seat(&qh, &seat);
    }

    while !app.exit {
        event_queue
            .blocking_dispatch(&mut app)
            .map_err(|e| format!("Wayland connection lost: {e}"))?;
        app.draw_if_needed(&qh);
    }
    Ok(())
}
