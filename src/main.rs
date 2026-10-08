//! blackhole-sand — entry point.
//!
//! Phase 1: opens a wgpu window, runs the sim, lets the user click to place a
//! planet and watch it get torn apart by the central black hole.

mod app;
mod material;
mod sim;
mod state;

use winit::event_loop::EventLoop;

fn main() {
    let event_loop = EventLoop::new().expect("create event loop");
    let mut app = app::App::new();
    event_loop
        .run_app(&mut app)
        .expect("event loop run");
}
