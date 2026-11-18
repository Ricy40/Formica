//! Phase 0 dummy agents: structure-of-arrays storage and a correlated random walk on the surface.
//!
//! These are placeholders that exercise storage, parallel systems, determinism and streaming. Real
//! ants (stages, castes, physiology, the three-layer controller) arrive in Phase 2, when the
//! `bevy_ecs` vs SoA benchmark decides the final storage.

use glam::Vec3;
use serde::{Deserialize, Serialize};

use crate::rng::{SystemId, entity_rng};
use crate::world::World;

/// Dummy walking speed range in voxels/s (2 mm voxels: 0.8 to 2.4 cm/s, the range of a walking
/// *Lasius* worker).
pub const SPEED_RANGE: (f32, f32) = (4.0, 12.0);
/// Heading diffusion of the correlated random walk, rad / sqrt(s).
pub const TURN_SD: f32 = 1.2;
/// Largest step up or down an agent will take, in voxels.
pub const MAX_STEP: f32 = 3.0;

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct Agents {
    pub id: Vec<u32>,
    pub pos: Vec<Vec3>,
    /// Heading in radians in `[0, 2π)`, measured from +X towards +Z.
    pub heading: Vec<f32>,
    pub speed: Vec<f32>,
    next_id: u32,
}

impl Agents {
    pub fn len(&self) -> usize {
        self.id.len()
    }

    pub fn is_empty(&self) -> bool {
        self.id.is_empty()
    }

    pub fn push(&mut self, pos: Vec3, heading: f32, speed: f32) -> u32 {
        let id = self.next_id;
        self.next_id += 1;
        self.id.push(id);
        self.pos.push(pos);
        self.heading.push(heading);
        self.speed.push(speed);
        id
    }

    pub fn clear(&mut self) {
        self.id.clear();
        self.pos.clear();
        self.heading.clear();
        self.speed.clear();
    }
}

const TAU: f32 = core::f32::consts::TAU;

/// Read-only inputs shared by every agent in one tick.
#[derive(Clone, Copy)]
struct TickCtx<'a> {
    world: &'a World,
    seed: u64,
    tick: u64,
    dt: f32,
}

#[inline]
fn step_one(ctx: TickCtx<'_>, id: u32, pos: &mut Vec3, heading: &mut f32, speed: f32) {
    let TickCtx {
        world,
        seed,
        tick,
        dt,
    } = ctx;
    let mut rng = entity_rng(seed, SystemId::AgentMove, tick, id as u64);
    let mut h = *heading + rng.approx_normal() * TURN_SD * dt.sqrt();
    let dir = Vec3::new(libm::cosf(h), 0.0, libm::sinf(h));
    let next = *pos + dir * speed * dt;
    let (cx, cz) = (libm::floorf(next.x) as i32, libm::floorf(next.z) as i32);
    match world.surface_at(cx, cz) {
        Some((height, top)) if top.is_solid() && (height as f32 - pos.y).abs() <= MAX_STEP => {
            *pos = Vec3::new(next.x, height as f32, next.z);
        }
        // Edge of the world, water or a cliff: turn around, with a little jitter.
        _ => h += core::f32::consts::PI + rng.range_f32(-0.5, 0.5),
    }
    let h = h.rem_euclid(TAU);
    // rem_euclid can round up to exactly TAU for tiny negative inputs.
    *heading = if h >= TAU { 0.0 } else { h };
}

/// Moves every agent one tick. Deterministic regardless of thread count: each agent reads only
/// the (immutable) world and its own counter-based RNG, and writes only its own slots.
pub fn step_dummies(agents: &mut Agents, world: &World, seed: u64, tick: u64, dt: f32) {
    let Agents {
        id,
        pos,
        heading,
        speed,
        ..
    } = agents;
    let ctx = TickCtx {
        world,
        seed,
        tick,
        dt,
    };

    #[cfg(feature = "parallel")]
    {
        use rayon::prelude::*;
        id.par_iter()
            .zip(pos.par_iter_mut())
            .zip(heading.par_iter_mut())
            .zip(speed.par_iter())
            .with_min_len(1024)
            .for_each(|(((&id, p), h), &s)| step_one(ctx, id, p, h, s));
    }
    #[cfg(not(feature = "parallel"))]
    {
        for (((&id, p), h), &s) in id
            .iter()
            .zip(pos.iter_mut())
            .zip(heading.iter_mut())
            .zip(speed.iter())
        {
            step_one(ctx, id, p, h, s);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::scene::build_test_chunk;

    #[test]
    fn ids_are_unique_and_monotonic() {
        let mut a = Agents::default();
        assert_eq!(a.push(Vec3::ZERO, 0.0, 1.0), 0);
        assert_eq!(a.push(Vec3::ZERO, 0.0, 1.0), 1);
        a.clear();
        assert_eq!(a.push(Vec3::ZERO, 0.0, 1.0), 2, "ids are never reused");
    }

    #[test]
    fn walkers_stay_on_surface_and_in_bounds() {
        let world = build_test_chunk();
        let mut a = Agents::default();
        for i in 0..50 {
            let (x, z) = (4 + (i % 10) * 2, 4 + (i / 10) * 2);
            let (h, _) = world.surface_at(x, z).unwrap();
            a.push(
                Vec3::new(x as f32 + 0.5, h as f32, z as f32 + 0.5),
                i as f32,
                8.0,
            );
        }
        for tick in 0..2000 {
            step_dummies(&mut a, &world, 9, tick, 1.0 / 30.0);
        }
        let ext = world.extent();
        for (p, h) in a.pos.iter().zip(&a.heading) {
            assert!(p.x >= 0.0 && p.z >= 0.0 && p.x < ext.size.x as f32 && p.z < ext.size.z as f32);
            let (sh, top) = world
                .surface_at(p.x.floor() as i32, p.z.floor() as i32)
                .unwrap();
            assert_eq!(p.y, sh as f32, "agent stands on the surface");
            assert!(top.is_solid(), "agent never walks on water");
            assert!((0.0..TAU).contains(h));
        }
    }

    #[test]
    fn walkers_actually_move() {
        let world = build_test_chunk();
        let mut a = Agents::default();
        let (h, _) = world.surface_at(10, 10).unwrap();
        let start = Vec3::new(10.5, h as f32, 10.5);
        a.push(start, 0.0, 8.0);
        for tick in 0..30 {
            step_dummies(&mut a, &world, 1, tick, 1.0 / 30.0);
        }
        assert!(a.pos[0].distance(start) > 1.0);
    }
}
