//! Phase 2 scenario loader: turn a `--bodies "..."` CLI string into a list
//! of bodies, or fall back to the default scenario when the flag is not
//! given.
//!
//! ## Body spec syntax
//!
//! Each spec is one of:
//!
//! * `bh:x=<x>,y=<y>,m=<mass>` — BlackHole, mass in mass units
//!   (1 mass unit = 1 Rock particle). x/y in grid cells.
//! * `planet:x=<x>,y=<y>,r=<radius>,vx=<x-vel>,vy=<y-vel>` — Planet. `r` is the
//!   spawn radius in cells. Particle count and body mass are both set to
//!   the spawn disk's cell count. Velocity is in cells/tick.
//!
//! Multiple specs are joined with `;`. Example:
//!
//! ```text
//! --bodies "bh:x=128,y=128,m=1000;planet:x=180,y=128,r=6,vx=0,vy=0.5"
//! ```
//!
//! See `docs/phase-2-multi-body-gravity.md` for the design rationale.

use crate::sim::World;

/// One parsed body spec, normalized. `World::spawn_*` consumes these.
#[derive(Debug, Clone, PartialEq)]
pub enum BodySpec {
    BlackHole { x: f32, y: f32, mass: f32 },
    Planet {
        x: f32,
        y: f32,
        radius: i32,
        vx: f32,
        vy: f32,
    },
}

#[derive(Debug, Clone, PartialEq)]
pub enum ScenarioError {
    EmptySpec,
    UnknownKind(String),
    MissingField(&'static str),
    BadValue { field: String, value: String },
    DuplicateField(String),
}

impl std::fmt::Display for ScenarioError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::EmptySpec => write!(f, "empty body spec"),
            Self::UnknownKind(s) => write!(f, "unknown body kind: {s:?} (expected 'bh' or 'planet')"),
            Self::MissingField(s) => write!(f, "missing required field: {s}"),
            Self::BadValue { field, value } => write!(f, "bad value for {field}: {value:?}"),
            Self::DuplicateField(s) => write!(f, "field appears twice: {s}"),
        }
    }
}

impl std::error::Error for ScenarioError {}

/// Parse the value of `--bodies "..."` (one full string, `;`-separated specs).
pub fn parse_bodies(spec: &str) -> Result<Vec<BodySpec>, ScenarioError> {
    let mut out = Vec::new();
    for part in spec.split(';') {
        let trimmed = part.trim();
        if trimmed.is_empty() {
            continue;
        }
        out.push(parse_one(trimmed)?);
    }
    Ok(out)
}

fn parse_one(spec: &str) -> Result<BodySpec, ScenarioError> {
    if spec.is_empty() {
        return Err(ScenarioError::EmptySpec);
    }
    let (kind, rest) = spec
        .split_once(':')
        .ok_or_else(|| ScenarioError::MissingField("kind"))?;
    let fields = parse_fields(rest)?;

    match kind.trim() {
        "bh" => {
            let x = fields.get("x").ok_or(ScenarioError::MissingField("x"))?;
            let y = fields.get("y").ok_or(ScenarioError::MissingField("y"))?;
            let m = fields.get("m").ok_or(ScenarioError::MissingField("m"))?;
            Ok(BodySpec::BlackHole { x: *x, y: *y, mass: *m })
        }
        "planet" => {
            let x = fields.get("x").ok_or(ScenarioError::MissingField("x"))?;
            let y = fields.get("y").ok_or(ScenarioError::MissingField("y"))?;
            let r = fields.get("r").ok_or(ScenarioError::MissingField("r"))?;
            let vx = fields.get("vx").copied().unwrap_or(0.0);
            let vy = fields.get("vy").copied().unwrap_or(0.0);
            Ok(BodySpec::Planet {
                x: *x,
                y: *y,
                radius: *r as i32,
                vx,
                vy,
            })
        }
        other => Err(ScenarioError::UnknownKind(other.to_string())),
    }
}

/// Parses the comma-separated `key=value,key=value` tail of a spec into
/// an ordered Vec so we can detect duplicate keys.
fn parse_fields(s: &str) -> Result<std::collections::HashMap<String, f32>, ScenarioError> {
    let mut out = std::collections::HashMap::new();
    for piece in s.split(',') {
        let piece = piece.trim();
        if piece.is_empty() {
            continue;
        }
        let (k, v) = piece
            .split_once('=')
            .ok_or_else(|| ScenarioError::BadValue { field: "field".to_string(), value: piece.to_string() })?;
        let k = k.trim();
        let v = v.trim();
        let parsed: f32 = v.parse().map_err(|_| ScenarioError::BadValue {
            field: k.to_string(),
            value: v.to_string(),
        })?;
        if out.insert(k.to_string(), parsed).is_some() {
            return Err(ScenarioError::DuplicateField(k.to_string()));
        }
    }
    Ok(out)
}

/// Default scenario: one BlackHole near grid center, one Planet on a
/// near-collision orbit so the user sees the slingshot + disintegration
/// immediately on `cargo run`. Tuned by hand:
/// * BH at grid center, mass 1000.
/// * Planet at distance 60 cells to the right of the BH, radius 6 cells
///   (~ 113 particles, mass=113), tangential velocity 0.5 cells/tick.
/// * 0.5 cells/tick is roughly the circular velocity at r=60: sqrt(G*M/r)
///   = sqrt(8e-3*1000/60) = sqrt(0.133) = 0.366 cells/tick. We're 1.4x
///   faster than circular, so the orbit is eccentric — periapsis drops
///   inside the tidal zone (~25 cells from BH) for several frames per
///   cycle, and the near-side rips off every pass.
pub fn default_scenario(world: &mut World) {
    world.spawn_black_hole(crate::sim::HOLE_X as f32, crate::sim::HOLE_Y as f32, 1000.0);
    world.spawn_planet(
        crate::sim::HOLE_X + 60,
        crate::sim::HOLE_Y,
        6,
        crate::material::Material::Rock as u8,
    );
    // Set the planet's initial velocity to a near-collision tangent.
    let planet_id = world.bodies.last().unwrap().id;
    if let Some(body) = world.bodies.iter_mut().find(|b| b.id == planet_id) {
        body.velocity = glam::Vec2::new(0.0, 0.5);
    }
}

/// Apply a parsed scenario to a fresh world.
pub fn apply_scenario(world: &mut World, specs: &[BodySpec]) {
    for spec in specs {
        match spec {
            BodySpec::BlackHole { x, y, mass } => {
                let _ = world.spawn_black_hole(*x, *y, *mass);
            }
            BodySpec::Planet { x, y, radius, vx, vy } => {
                let pid = world.spawn_planet(
                    x.round() as i32,
                    y.round() as i32,
                    *radius,
                    crate::material::Material::Rock as u8,
                );
                // Override the planet body's velocity (spawn_planet
                // sets it to 0). Also sync the body's position to the
                // particle disk center for clean physics.
                if pid != crate::sim::NO_BODY as u32 {
                    if let Some(b) = world.bodies.iter_mut().find(|b| b.id == pid) {
                        b.position = glam::Vec2::new(*x, *y);
                        b.velocity = glam::Vec2::new(*vx, *vy);
                    }
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn close(a: f32, b: f32) {
        assert!((a - b).abs() < 1e-4, "{a} != {b}");
    }

    #[test]
    fn parses_single_bh() {
        let v = parse_bodies("bh:x=128,y=128,m=1000").unwrap();
        assert_eq!(
            v,
            vec![BodySpec::BlackHole { x: 128.0, y: 128.0, mass: 1000.0 }]
        );
    }

    #[test]
    fn parses_planet_with_velocity() {
        let v = parse_bodies("planet:x=180,y=128,r=6,vx=0,vy=0.5").unwrap();
        assert_eq!(
            v,
            vec![BodySpec::Planet {
                x: 180.0,
                y: 128.0,
                radius: 6,
                vx: 0.0,
                vy: 0.5,
            }]
        );
    }

    #[test]
    fn parses_multiple_specs() {
        let s = "bh:x=128,y=128,m=1000; planet:x=180,y=128,r=6,vx=0,vy=0.5";
        let v = parse_bodies(s).unwrap();
        assert_eq!(v.len(), 2);
    }

    #[test]
    fn planet_defaults_velocity_to_zero() {
        let v = parse_bodies("planet:x=180,y=128,r=6").unwrap();
        assert_eq!(
            v[0],
            BodySpec::Planet { x: 180.0, y: 128.0, radius: 6, vx: 0.0, vy: 0.0 }
        );
    }

    #[test]
    fn rejects_unknown_kind() {
        match parse_bodies("star:x=128,y=128,m=1000") {
            Err(ScenarioError::UnknownKind(s)) => assert_eq!(s, "star"),
            other => panic!("expected UnknownKind, got {other:?}"),
        }
    }

    #[test]
    fn rejects_missing_required_field() {
        match parse_bodies("planet:x=180,y=128") {
            Err(ScenarioError::MissingField("r")) => {}
            other => panic!("expected MissingField(\"r\"), got {other:?}"),
        }
    }

    #[test]
    fn rejects_bad_value() {
        match parse_bodies("bh:x=128,y=128,m=abc") {
            Err(ScenarioError::BadValue { field, .. }) => assert_eq!(field, "m"),
            other => panic!("expected BadValue, got {other:?}"),
        }
    }

    #[test]
    fn rejects_duplicate_field() {
        match parse_bodies("bh:x=128,y=128,m=1000,m=2000") {
            Err(ScenarioError::DuplicateField(ref s)) if s == "m" => {}
            other => panic!("expected DuplicateField(\"m\"), got {other:?}"),
        }
    }

    #[test]
    fn empty_string_yields_empty_vec() {
        assert_eq!(parse_bodies("").unwrap(), Vec::new());
    }

    #[test]
    fn apply_scenario_creates_bodies() {
        let mut w = World::new();
        let specs = parse_bodies("bh:x=128,y=128,m=1000;planet:x=180,y=128,r=6,vx=0,vy=0.5").unwrap();
        apply_scenario(&mut w, &specs);
        assert_eq!(w.bodies.len(), 2);
        // BlackHole is first, Planet is second.
        assert!(w.bodies[0].destroys_particles());
        assert!(!w.bodies[1].destroys_particles());
        close(w.bodies[1].velocity.y, 0.5);
    }
}
