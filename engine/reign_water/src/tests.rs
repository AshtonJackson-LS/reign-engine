use super::*;

const FRAME: f32 = 1.0 / 60.0;

fn pool() -> Water {
    Water::new(0.0, 100.0, 320.0, 48.0)
}

fn run(water: &mut Water, seconds: f32) {
    for _ in 0..(seconds / FRAME).round() as usize {
        water.step(FRAME);
    }
}

fn peak(water: &Water, from: f32, to: f32) -> f32 {
    let mut x = from;
    let mut most = 0.0f32;
    while x <= to {
        most = most.max((water.surface(x) - water.y).abs());
        x += 1.0;
    }
    most
}

#[test]
fn still_water_stays_flat() {
    let mut water = pool();
    run(&mut water, 2.0);
    assert!(water.calm());
    assert_eq!(water.surface(160.0), 100.0);
}

#[test]
fn a_splash_travels_out_both_ways_and_settles() {
    let mut water = pool();
    water.splash(160.0, 300.0, 10.0);
    assert!(water.droplet_count() > 0, "a hard splash throws droplets");
    run(&mut water, 0.05);
    assert!(water.surface(160.0) > 100.5, "the surface is pushed down where it's hit");
    run(&mut water, 0.3);
    // The ripple has reached both sides, a good way from the splash.
    assert!(peak(&water, 60.0, 100.0) > 0.2, "left: {}", peak(&water, 60.0, 100.0));
    assert!(peak(&water, 220.0, 260.0) > 0.2, "right: {}", peak(&water, 220.0, 260.0));
    // Symmetric, as the splash was.
    let (l, r) = (water.surface(120.0), water.surface(200.0));
    assert!((l - r).abs() < 0.05, "{l} vs {r}");
    run(&mut water, 8.0);
    assert!(water.calm(), "everything settles");
}

#[test]
fn surface_stays_within_its_limit() {
    let mut water = pool();
    for _ in 0..50 {
        water.splash(100.0, 5000.0, 20.0);
        water.step(FRAME);
    }
    for x in 0..=320 {
        let h = water.surface(x as f32) - water.y;
        assert!(h.abs() <= water.ripples.limit + 1e-3 && h.is_finite(), "{h} at {x}");
    }
}

#[test]
fn long_frames_are_split_and_stay_stable() {
    let mut a = pool();
    let mut b = pool();
    a.push(160.0, 200.0, 8.0);
    b.push(160.0, 200.0, 8.0);
    // One long frame, clamped to 0.1 s, against six short ones.
    a.step(0.1);
    for _ in 0..6 {
        b.step(0.1 / 6.0);
    }
    for x in [140.0, 160.0, 190.0] {
        assert!((a.surface(x) - b.surface(x)).abs() < 0.05, "{x}");
        assert!(a.surface(x).is_finite());
    }
}

#[test]
fn a_body_falling_in_splashes_and_one_jumping_out_pulls_the_surface_up() {
    let mut water = pool();
    // Feet 20 px above the surface, then 6 px under it a frame later.
    water.touch(1, 150.0, 60.0, 16.0, 20.0);
    water.step(FRAME);
    assert!(water.calm(), "nothing in the water yet");
    water.touch(1, 150.0, 86.0, 16.0, 20.0);
    water.step(FRAME);
    assert!(water.droplet_count() > 0, "a splash on entry");
    assert!(water.surface(158.0) > 100.0, "pushed down");

    let mut water = pool();
    water.touch(1, 150.0, 86.0, 16.0, 20.0);
    water.step(FRAME);
    water.touch(1, 150.0, 70.0, 16.0, 20.0);
    water.step(FRAME);
    run(&mut water, 0.02);
    assert!(water.surface(158.0) < 100.0, "pulled up after the body");
}

#[test]
fn wading_pushes_a_ripple_ahead() {
    let mut water = pool();
    // Standing waist deep, then walking right at 90 px/s.
    let mut x = 40.0;
    for _ in 0..30 {
        water.touch(7, x, 90.0, 16.0, 20.0);
        water.step(FRAME);
        x += 90.0 * FRAME;
    }
    let ahead = peak(&water, x + 16.0, x + 40.0);
    assert!(ahead > 0.3, "a bow wave ahead: {ahead}");
    assert!(ahead < 4.0, "but not a wall of water: {ahead}");
    // Standing still makes no new waves: it all settles.
    for _ in 0..600 {
        water.touch(7, x, 90.0, 16.0, 20.0);
        water.step(FRAME);
    }
    assert!(water.calm());
}

#[test]
fn bodies_not_touched_are_forgotten() {
    let mut water = pool();
    water.touch(1, 150.0, 60.0, 16.0, 20.0);
    water.step(FRAME);
    // Gone for a frame, then back far below: no splash, it's a new body.
    water.step(FRAME);
    water.touch(1, 150.0, 90.0, 16.0, 20.0);
    water.step(FRAME);
    assert!(water.calm());
}

#[test]
fn droplets_fall_back_in() {
    let mut water = pool();
    water.splash(160.0, 400.0, 8.0);
    assert!(water.droplet_count() > 0);
    run(&mut water, 3.0);
    assert_eq!(water.droplet_count(), 0);
}

#[test]
fn outside_the_water_is_its_resting_surface() {
    let mut water = pool();
    water.splash(0.0, 500.0, 30.0);
    run(&mut water, 0.05);
    assert_eq!(water.surface(-10.0), 100.0);
    assert!(!water.contains(-10.0, 120.0));
    assert!(water.contains(200.0, 120.0));
    assert!(!water.contains(200.0, 160.0), "under the bottom");
}
