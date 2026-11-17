//! The particle effects of a scene's [`ParticleSystem`] components: started
//! when their object appears (or a script plays them), kept on their
//! object, and drawn over the scene.
//!
//! This is drawing, not simulation. The scene only holds what scripts set
//! (`plays`, `emitting`); the particles themselves live here, step in real
//! time and hit the scene's solid tilemaps. Play/Stop and a new scene start
//! everything afresh ([`SceneParticles::reset`]).

use std::collections::HashMap;
use std::time::Instant;

use reign::assets::{AssetDb, Handle, ParticleEffect};
use reign::particles::{Effects, Ground, ParticleRenderer, ParticleWorld, Spawn, SystemId};
use reign::render::Renderer;
use reign::scene::{ObjectId, Scene};

use crate::components::ParticleSystem;
use crate::physics::SolidTiles;

/// What a [`ParticleSystem`] last asked for, and the effect playing for it.
#[derive(Clone, Copy, Debug)]
struct Tracked {
    effect: Handle<ParticleEffect>,
    plays: u32,
    system: Option<SystemId>,
}

#[derive(Default)]
pub struct SceneParticles {
    world: ParticleWorld,
    renderer: ParticleRenderer,
    tracked: HashMap<ObjectId, Tracked>,
    /// When the last frame was drawn, for the time between frames.
    last: Option<Instant>,
}

impl SceneParticles {
    /// Forgets every effect and particle: a new scene, or Play/Stop.
    pub fn reset(&mut self) {
        self.world.clear();
        self.tracked.clear();
        self.last = None;
    }

    /// Starts, moves and stops effects to match the scene's
    /// [`ParticleSystem`]s, then steps them by `dt` seconds.
    pub fn sync(&mut self, scene: &Scene, effects: &dyn Effects, dt: f32, ground: &dyn Ground) {
        let Self { world, tracked, .. } = self;
        let mut seen = Vec::new();
        for (id, ps) in scene.iter::<ParticleSystem>() {
            seen.push(id);
            let p = scene.world_position(id);
            let flip = scene.transform(id).is_some_and(|t| t.flip_x);
            let spawn = Spawn::at(p.x.to_f32(), p.y.to_f32()).flip_x(flip);
            let play = |world: &mut ParticleWorld| {
                (!ps.effect.is_none()).then(|| world.play(effects, ps.effect, spawn)).flatten()
            };
            let t = match tracked.get_mut(&id) {
                None => {
                    // New: it starts if it's set to.
                    let system = (ps.play_on_start && ps.emitting).then(|| play(world)).flatten();
                    tracked.insert(id, Tracked { effect: ps.effect, plays: ps.plays, system });
                    continue;
                }
                Some(t) => t,
            };
            if t.effect != ps.effect || t.plays != ps.plays {
                if let Some(old) = t.system {
                    if t.effect == ps.effect {
                        // Played again: the old particles finish.
                        world.stop(old);
                    } else {
                        world.remove(old);
                    }
                }
                t.system = ps.emitting.then(|| play(world)).flatten();
                (t.effect, t.plays) = (ps.effect, ps.plays);
            }
            if let Some(system) = t.system {
                if !ps.emitting {
                    world.stop(system);
                }
                world.move_to(system, spawn.x, spawn.y);
                if let Some(s) = world.spawn_mut(system) {
                    s.flip_x = flip;
                }
            }
        }
        // Objects that are gone: their particles finish.
        tracked.retain(|id, t| {
            let keep = seen.contains(id);
            if !keep && let Some(system) = t.system {
                world.stop(system);
            }
            keep
        });
        world.step(dt, effects, ground);
    }

    /// Steps by the time since the last frame and queues the particles,
    /// over everything drawn so far.
    pub fn draw(&mut self, r: &mut Renderer, scene: &Scene, db: &AssetDb) {
        let now = Instant::now();
        let dt = self.last.map_or(0.0, |last| now.duration_since(last).as_secs_f32());
        self.last = Some(now);
        // Nothing to do in a scene without particles (most of them).
        if self.tracked.is_empty()
            && self.world.particle_count() == 0
            && scene.iter::<ParticleSystem>().next().is_none()
        {
            return;
        }
        let solid = SolidTiles::of(scene);
        self.sync(scene, db, dt, &|x: f32, y: f32| solid.solid(x, y));
        self.renderer.draw(r, db, &self.world);
        self.renderer.lights(r, &self.world);
    }

    /// Particles alive.
    #[cfg(test)]
    pub fn particle_count(&self) -> usize {
        self.world.particle_count()
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use reign::assets::particles::{Burst, CountRange, Emission, Main, Range};
    use reign::assets::{Guid, Handle, ParticleEffect};
    use reign::core::FxVec2;
    use reign::particles::{Effects, no_ground};
    use reign::scene::Scene;

    use super::*;

    const FRAME: f32 = 1.0 / 60.0;

    /// A one-shot burst of 4 particles living a second, as asset 1.
    struct OneEffect(Arc<ParticleEffect>);

    impl Effects for OneEffect {
        fn effect(&self, handle: Handle<ParticleEffect>) -> Option<Arc<ParticleEffect>> {
            (handle.guid == Guid(1)).then(|| self.0.clone())
        }
    }

    fn effects() -> OneEffect {
        OneEffect(Arc::new(ParticleEffect {
            main: Main {
                looping: false,
                duration: 0.1,
                lifetime: Range(1.0, 1.0),
                speed: Range(0.0, 0.0),
                ..Main::default()
            },
            emission: Emission {
                rate: 0.0,
                bursts: vec![Burst { count: CountRange(4, 4), ..Burst::default() }],
                ..Emission::default()
            },
            ..ParticleEffect::default()
        }))
    }

    fn with_system(scene: &mut Scene, play_on_start: bool) -> ObjectId {
        let id = scene.spawn("Sparks");
        scene.insert(
            id,
            ParticleSystem {
                effect: Handle::new(Guid(1)),
                play_on_start,
                ..ParticleSystem::default()
            },
        );
        id
    }

    fn step(p: &mut SceneParticles, scene: &Scene, fx: &OneEffect) {
        p.sync(scene, fx, FRAME, &no_ground);
    }

    #[test]
    fn a_system_plays_on_start_follows_its_object_and_scripts_replay_it() {
        let fx = effects();
        let mut scene = Scene::new();
        let id = with_system(&mut scene, true);
        let mut p = SceneParticles::default();
        step(&mut p, &scene, &fx);
        step(&mut p, &scene, &fx);
        assert_eq!(p.particle_count(), 4, "played when it appeared");

        scene.set_world_position(id, FxVec2::from_px(50, 7));
        step(&mut p, &scene, &fx);
        let system = p.tracked[&id].system.unwrap();
        let spawn = *p.world.spawn_mut(system).unwrap();
        assert_eq!((spawn.x, spawn.y), (50.0, 7.0), "the emitter follows the object");

        // A script plays it again: the first particles keep going, four more.
        scene.get_mut::<ParticleSystem>(id).unwrap().plays += 1;
        step(&mut p, &scene, &fx);
        step(&mut p, &scene, &fx);
        assert_eq!(p.particle_count(), 8);
    }

    #[test]
    fn without_play_on_start_it_waits_for_a_script() {
        let fx = effects();
        let mut scene = Scene::new();
        let id = with_system(&mut scene, false);
        let mut p = SceneParticles::default();
        for _ in 0..5 {
            step(&mut p, &scene, &fx);
        }
        assert_eq!(p.particle_count(), 0);
        scene.get_mut::<ParticleSystem>(id).unwrap().plays += 1;
        step(&mut p, &scene, &fx);
        step(&mut p, &scene, &fx);
        assert_eq!(p.particle_count(), 4);
    }

    #[test]
    fn removed_objects_particles_finish_and_reset_clears_everything() {
        let fx = effects();
        let mut scene = Scene::new();
        let id = with_system(&mut scene, true);
        let mut p = SceneParticles::default();
        step(&mut p, &scene, &fx);
        step(&mut p, &scene, &fx);
        scene.despawn(id);
        step(&mut p, &scene, &fx);
        assert_eq!(p.particle_count(), 4, "still finishing");
        assert!(p.tracked.is_empty());
        for _ in 0..70 {
            step(&mut p, &scene, &fx);
        }
        assert_eq!(p.particle_count(), 0);

        let id = with_system(&mut scene, true);
        step(&mut p, &scene, &fx);
        step(&mut p, &scene, &fx);
        p.reset();
        assert_eq!(p.particle_count(), 0);
        // The object is seen as new again, so it plays again.
        step(&mut p, &scene, &fx);
        step(&mut p, &scene, &fx);
        assert_eq!(p.particle_count(), 4);
        assert!(p.tracked.contains_key(&id));
    }

    #[test]
    fn no_effect_or_a_missing_one_plays_nothing() {
        let fx = effects();
        let mut scene = Scene::new();
        let id = with_system(&mut scene, true);
        scene.get_mut::<ParticleSystem>(id).unwrap().effect = Handle::NONE;
        let other = scene.spawn("Missing");
        scene.insert(
            other,
            ParticleSystem { effect: Handle::new(Guid(99)), ..ParticleSystem::default() },
        );
        let mut p = SceneParticles::default();
        step(&mut p, &scene, &fx);
        step(&mut p, &scene, &fx);
        assert_eq!(p.particle_count(), 0);
    }

    #[test]
    fn particles_hit_the_scenes_solid_tilemaps_only() {
        use crate::components::TilemapRenderer;
        use crate::physics::TilemapCollider;
        use reign::assets::Tilemap;
        let map = Arc::new(Tilemap::from_ascii(
            "....
..#.
####
",
        ));
        let mut scene = Scene::new();
        let ground = scene.spawn("Ground");
        scene.set_world_position(ground, FxVec2::from_px(100, 200));
        scene.insert(
            ground,
            TilemapRenderer { tilemap: Handle::new(Guid(5)), ..Default::default() },
        );
        scene.insert(ground, TilemapCollider::default());
        let solid = SolidTiles::with(&scene, &|_| Some(map.clone()));
        assert!(!solid.solid(110.0, 210.0), "an empty cell");
        assert!(solid.solid(140.0, 220.0), "the block in the middle row");
        assert!(solid.solid(100.0, 232.0), "the floor row");
        assert!(!solid.solid(99.0, 232.0) && !solid.solid(100.0, 249.0), "outside the map");
        scene.insert(ground, TilemapCollider { is_trigger: true, ..TilemapCollider::default() });
        let trigger = SolidTiles::with(&scene, &|_| Some(map.clone()));
        assert!(!trigger.solid(100.0, 232.0), "a trigger isn't solid");
    }
}
