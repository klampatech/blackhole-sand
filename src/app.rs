//! winit ApplicationHandler. Owns the wgpu `State` and the sim `World`.
//!
//! ## Body construction
//!
//! Phase 2: App::new accepts an optional scenario. When supplied, the
//! bodies from the scenario replace the default. When not, we use the
//! default scenario (1 BH at grid center + 1 planet on a circular
//! orbit, COM-stationary init — see SPEC decision #20) and also seed
//! a thin rain of particles so the user sees the falling-sand effect.
//!
//! ## Input mapping (Phase 2)
//!   * Left-click  — spawn a new Rock planet at the cursor (added to the
//!                   sim, not replacing existing bodies).
//!   * Right-click — clear all particles AND bodies.
//!   * Window resize — reconfigure the swap chain.
//!   * Close / Cmd-Q — quit.

use std::sync::Arc;

use crate::material::Material;
use crate::scenario::{self, BodySpec};
use crate::sim::{World, H, W};
use crate::state::State;
use winit::application::ApplicationHandler;
use winit::dpi::PhysicalPosition;
use winit::event::{ElementState, MouseButton, WindowEvent};
use winit::event_loop::ActiveEventLoop;
use winit::window::{Window, WindowAttributes, WindowId};

/// State we need to track for the click-to-spawn interaction.
#[derive(Default)]
struct InputState {
    cursor_phys: Option<PhysicalPosition<f64>>,
}

pub struct App {
    state: Option<State>,
    world: World,
    input: InputState,
}

impl App {
    /// Build the app. If `scenario` is `Some`, use those body specs.
    /// If `None`, build the default scenario (1 BH + 1 planet) and
    /// additionally seed a thin rain of particles so the falling-sand
    /// effect is visible at startup.
    pub fn new(scenario: Option<&[BodySpec]>) -> Self {
        let mut world = World::new();
        match scenario {
            Some(specs) => scenario::apply_scenario(&mut world, specs),
            None => {
                scenario::default_scenario(&mut world);
                world.seed_rain();
            }
        }
        Self {
            state: None,
            world,
            input: InputState::default(),
        }
    }

    /// Map a physical pixel position to a grid cell, with letterboxing.
    fn cursor_to_cell(&self, pos: PhysicalPosition<f64>) -> Option<(i32, i32)> {
        let st = self.state.as_ref()?;
        let win = st.window.inner_size();
        if win.width == 0 || win.height == 0 {
            return None;
        }
        let (sx, sy) = if win.width <= win.height {
            let scale = win.width as f64 / W as f64;
            let used_h = scale * H as f64;
            let y_off = (win.height as f64 - used_h) * 0.5;
            (pos.x / scale, (pos.y - y_off) / scale)
        } else {
            let scale = win.height as f64 / H as f64;
            let used_w = scale * W as f64;
            let x_off = (win.width as f64 - used_w) * 0.5;
            ((pos.x - x_off) / scale, pos.y / scale)
        };
        if sx < 0.0 || sy < 0.0 || sx >= W as f64 || sy >= H as f64 {
            return None;
        }
        Some((sx as i32, sy as i32))
    }

    fn spawn_planet_at_cursor(&mut self) {
        if let Some(pos) = self.input.cursor_phys {
            if let Some((cx, cy)) = self.cursor_to_cell(pos) {
                self.world.spawn_planet(cx, cy, 10, Material::Rock as u8);
            }
        }
    }

    fn clear_world(&mut self) {
        self.world = World::new();
    }
}

impl ApplicationHandler for App {
    fn resumed(&mut self, event_loop: &ActiveEventLoop) {
        if self.state.is_some() {
            return;
        }
        let attrs = WindowAttributes::default()
            .with_title("blackhole-sand")
            .with_inner_size(winit::dpi::LogicalSize::new(768.0, 768.0));
        let window: Arc<Window> = Arc::new(event_loop.create_window(attrs).expect("create window"));
        let state = State::new(window);
        self.state = Some(state);
        self.state.as_ref().unwrap().window.request_redraw();
    }

    fn window_event(
        &mut self,
        event_loop: &ActiveEventLoop,
        _id: WindowId,
        event: WindowEvent,
    ) {
        let Some(state) = self.state.as_mut() else {
            return;
        };
        match event {
            WindowEvent::CloseRequested => event_loop.exit(),

            WindowEvent::Resized(sz) => state.resize(sz.width, sz.height),

            WindowEvent::CursorMoved { position, .. } => {
                self.input.cursor_phys = Some(position);
            }

            WindowEvent::MouseInput {
                state: ElementState::Pressed,
                button,
                ..
            } => match button {
                MouseButton::Left => self.spawn_planet_at_cursor(),
                MouseButton::Right => self.clear_world(),
                _ => {}
            }

            WindowEvent::RedrawRequested => {
                self.world.step();
                match state.render(&self.world) {
                    Ok(()) => {}
                    Err(wgpu::SurfaceError::Lost | wgpu::SurfaceError::Outdated) => {
                        let s = state.window.inner_size();
                        state.resize(s.width, s.height);
                    }
                    Err(e) => {
                        eprintln!("render error: {e}");
                        event_loop.exit();
                    }
                }
                state.window.request_redraw();
            }

            _ => {}
        }
    }
}
