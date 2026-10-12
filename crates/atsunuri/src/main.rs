mod app;
mod decorations;
mod renderer;
mod tablet;
mod view;

use atsunuri_core::Canvas;
use wayland_client::{Connection, globals::registry_queue_init};

use crate::app::App;

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

    let canvas = Canvas::new(CANVAS_SIZE.0, CANVAS_SIZE.1);
    let mut app = App::new(&conn, &globals, &qh, canvas)?;

    while !app.exit {
        event_queue
            .blocking_dispatch(&mut app)
            .map_err(|e| format!("Wayland connection lost: {e}"))?;
        app.draw_if_needed(&conn, &qh);
    }
    Ok(())
}
