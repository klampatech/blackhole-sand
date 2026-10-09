//! blackhole-sand — entry point.
//!
//! Phase 1: opens a wgpu window, runs the sim, lets the user click to place a
//! planet and watch it get torn apart by the central black hole.
//!
//! Phase 2: accepts a `--bodies "..."` CLI spec to override the default
//! scenario. See `src/scenario.rs` for the spec syntax.

mod app;
mod body;
mod material;
mod scenario;
mod sim;
mod state;

use winit::event_loop::EventLoop;

/// Parse `--bodies` from CLI args. Anything else (or no args) returns
/// `None`, which means "use the default scenario" in `App::new`.
///
/// Two forms accepted:
///   --bodies "..."
///   --bodies=...
fn parse_cli() -> Option<Vec<scenario::BodySpec>> {
    let mut args = std::env::args().skip(1);
    while let Some(arg) = args.next() {
        if arg == "--bodies" {
            let val = args.next().expect("--bodies requires a value");
            return Some(
                scenario::parse_bodies(&val)
                    .unwrap_or_else(|e| panic!("bad --bodies spec: {e}")),
            );
        }
        if let Some(rest) = arg.strip_prefix("--bodies=") {
            return Some(
                scenario::parse_bodies(rest)
                    .unwrap_or_else(|e| panic!("bad --bodies spec: {e}")),
            );
        }
    }
    None
}

fn main() {
    let event_loop = EventLoop::new().expect("create event loop");
    let scenario = parse_cli();
    let mut app = app::App::new(scenario.as_deref());
    event_loop
        .run_app(&mut app)
        .expect("event loop run");
}
