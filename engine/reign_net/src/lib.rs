//! Rollback netcode for the Reign engine, on top of [ggrs].
//!
//! The game hands over a [`Simulation`]: something that can be cloned, stepped
//! with one [`InputFrame`] per player, and fingerprinted. This crate turns
//! ggrs's save, load and advance requests into those calls, so the game
//! loop never touches ggrs types directly.
//!
//! Each tick a session asks for input from the local player, predicts the
//! remote one, and when the real remote input arrives late it loads an
//! older snapshot and re-simulates. Rendering only ever sees the newest
//! state, so a rollback shows as a small correction, not a stall.

pub mod lobby;
pub mod sim_net;

use std::marker::PhantomData;
use std::net::SocketAddr;

use ggrs::{
    Config, DesyncDetection, GgrsRequest, InputStatus, P2PSession, PlayerType, PredictRepeatLast,
    SessionBuilder, SyncTestSession,
};
use reign_core::{InputFrame, TICK_HZ};

/// A non-blocking UDP socket ggrs can talk through.
pub use ggrs::UdpNonBlockingSocket as Udp;
pub use ggrs::{GgrsError, GgrsEvent, Message, NonBlockingSocket, SessionState};
pub use lobby::{Lobby, Match};

/// A deterministic game state that can be rolled back.
pub trait Simulation: Clone + 'static {
    /// Advances one tick with each player's buttons, in player order.
    fn step(&mut self, inputs: &[InputFrame]);
    /// A fingerprint of the whole state. Peers compare these to catch desyncs.
    fn checksum(&self) -> u64;
}

/// The ggrs configuration for a [`Simulation`]. Inputs go over the wire as
/// the raw button bits.
pub struct Net<S>(PhantomData<S>);

impl<S: Simulation> Config for Net<S> {
    type Input = u16;
    type InputPredictor = PredictRepeatLast;
    type State = S;
    type Address = SocketAddr;
}

/// Ticks of input delay. At 120 Hz, two ticks is about 17 ms, which hides
/// most of a short hop's latency without the controls feeling late.
pub const INPUT_DELAY: usize = 2;
/// How far ahead of the last confirmed remote input the game may predict.
/// ggrs's default of 8 assumes 60 Hz; at 120 Hz this keeps ~133 ms.
pub const MAX_PREDICTION: usize = 16;
/// Ticks between checksum exchanges with the peer.
pub const DESYNC_INTERVAL: u32 = 30;

/// Who plays in a session, in player order.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Seat {
    /// This machine.
    Local,
    /// A peer at this address.
    Remote(SocketAddr),
}

/// A two-way session with peers over UDP.
pub type Session<S> = P2PSession<Net<S>>;

/// Opens a UDP socket on `port`, for [`start`].
pub fn udp(port: u16) -> Result<Udp, GgrsError> {
    Udp::bind_to_port(port).map_err(|e| GgrsError::InvalidRequest {
        info: format!("could not open UDP port {port}: {e}"),
    })
}

/// Starts a peer-to-peer session listening on `port`.
pub fn connect<S: Simulation>(port: u16, seats: &[Seat]) -> Result<Session<S>, GgrsError> {
    start(udp(port)?, seats)
}

/// Starts a peer-to-peer session over any socket, such as a UDP one
/// wrapped to add lag, or a [`sim_net::SimSocket`] in tests.
pub fn start<S: Simulation>(
    socket: impl NonBlockingSocket<SocketAddr> + 'static,
    seats: &[Seat],
) -> Result<Session<S>, GgrsError> {
    let mut builder = builder::<S>(seats.len())?
        .with_desync_detection_mode(DesyncDetection::On { interval: DESYNC_INTERVAL });
    for (handle, seat) in seats.iter().enumerate() {
        let kind = match *seat {
            Seat::Local => PlayerType::Local,
            Seat::Remote(addr) => PlayerType::Remote(addr),
        };
        builder = builder.add_player(kind, handle)?;
    }
    builder.start_p2p_session(socket)
}

/// A local session that rolls back `check_distance` ticks every tick and
/// re-simulates, failing if the replayed state differs. This is how CI
/// proves the simulation is safe to roll back.
pub fn sync_test<S: Simulation>(
    players: usize,
    check_distance: usize,
) -> Result<SyncTestSession<Net<S>>, GgrsError> {
    builder::<S>(players)?.with_check_distance(check_distance).start_synctest_session()
}

fn builder<S: Simulation>(players: usize) -> Result<SessionBuilder<Net<S>>, GgrsError> {
    Ok(SessionBuilder::new()
        .with_num_players(players)?
        .with_fps(TICK_HZ as usize)?
        .with_input_delay(INPUT_DELAY)
        .with_max_prediction_window(MAX_PREDICTION))
}

/// Carries out what a session asked for, in order. Returns how many ticks
/// were simulated, re-simulation included.
pub fn fulfil<S: Simulation>(sim: &mut S, requests: Vec<GgrsRequest<Net<S>>>) -> u32 {
    let mut ticks = 0;
    for request in requests {
        match request {
            GgrsRequest::SaveGameState { cell, frame } => {
                let sum = sim.checksum() as u128;
                cell.save(frame, Some(sim.clone()), Some(sum));
            }
            GgrsRequest::LoadGameState { cell, .. } => {
                *sim = cell.load().expect("ggrs only loads states it asked to save");
            }
            GgrsRequest::AdvanceFrame { inputs } => {
                let frames: Vec<InputFrame> = inputs
                    .iter()
                    .map(|&(bits, status)| match status {
                        // A player who left stands still.
                        InputStatus::Disconnected => InputFrame::default(),
                        _ => InputFrame::from_bits(bits),
                    })
                    .collect();
                sim.step(&frames);
                ticks += 1;
            }
        }
    }
    ticks
}

#[cfg(test)]
mod tests {
    use super::*;
    use reign_core::{Button, Rng, checksum};

    /// A tiny stand-in game: each player's counter moves with Left/Right,
    /// and an RNG ticks along, so a missed restore shows up.
    #[derive(Clone, Debug, Hash)]
    struct Counters {
        values: Vec<i32>,
        rng: Rng,
    }

    impl Simulation for Counters {
        fn step(&mut self, inputs: &[InputFrame]) {
            for (v, f) in self.values.iter_mut().zip(inputs) {
                *v += f.axis_x() + self.rng.range(0, 1);
            }
        }
        fn checksum(&self) -> u64 {
            checksum(self)
        }
    }

    fn input(player: usize, tick: u32) -> u16 {
        let mut f = InputFrame::default();
        f.set(
            if (tick / 7 + player as u32).is_multiple_of(2) { Button::Left } else { Button::Right },
            true,
        );
        f.bits()
    }

    #[test]
    fn sync_test_passes_for_a_deterministic_sim() {
        let mut session = sync_test::<Counters>(2, 6).unwrap();
        let mut sim = Counters { values: vec![0, 0], rng: Rng::new(9) };
        for tick in 0..200 {
            for p in 0..2 {
                session.add_local_input(p, input(p, tick)).unwrap();
            }
            fulfil(&mut sim, session.advance_frame().unwrap());
        }
    }

    /// A sim that keeps hidden state outside what it saves, like a static
    /// counter, must be caught.
    #[test]
    fn sync_test_catches_state_that_is_not_rolled_back() {
        use std::cell::Cell;
        thread_local!(static LEAK: Cell<i32> = const { Cell::new(0) });

        #[derive(Clone, Hash)]
        struct Leaky(i32);
        impl Simulation for Leaky {
            fn step(&mut self, _: &[InputFrame]) {
                LEAK.with(|l| l.set(l.get() + 1));
                self.0 += LEAK.with(Cell::get);
            }
            fn checksum(&self) -> u64 {
                checksum(self)
            }
        }

        let mut session = sync_test::<Leaky>(1, 4).unwrap();
        let mut sim = Leaky(0);
        let caught = (0..60).any(|_| {
            session.add_local_input(0, 0).unwrap();
            match session.advance_frame() {
                Ok(requests) => {
                    fulfil(&mut sim, requests);
                    false
                }
                Err(GgrsError::MismatchedChecksum { .. }) => true,
                Err(e) => panic!("{e}"),
            }
        });
        assert!(caught, "the sync test should flag the leak");
    }

    /// Two peers over a simulated link with 40 ms of latency, 20 ms of
    /// jitter and one packet in ten lost. Every tick both have confirmed
    /// must have the same state, and late inputs must have rolled back.
    #[test]
    fn peers_agree_over_a_lossy_jittery_link() {
        use sim_net::{Conditions, SimNetwork};
        use std::collections::BTreeMap;
        use std::time::{Duration, Instant};

        let ms = Duration::from_millis;
        let net =
            SimNetwork::new(Conditions { latency: ms(40), jitter: ms(20), loss_percent: 10 }, 7);
        let mut peers: Vec<_> = (0..2)
            .map(|me| {
                let other = SimNetwork::addr(1 - me as u16);
                let seats: Vec<_> = (0..2)
                    .map(|p| if p == me { Seat::Local } else { Seat::Remote(other) })
                    .collect();
                let session = start::<Counters>(net.socket(me as u16), &seats).unwrap();
                let sim = Counters { values: vec![0, 0], rng: Rng::new(9) };
                (session, sim, BTreeMap::new(), 0u32)
            })
            .collect();
        let target = 300;
        let deadline = Instant::now() + Duration::from_secs(60);
        while peers.iter().any(|p| p.0.confirmed_frame() < target) {
            assert!(Instant::now() < deadline, "peers never reached tick {target}");
            for (me, (session, sim, sums, rollbacks)) in peers.iter_mut().enumerate() {
                session.poll_remote_clients();
                for event in session.events() {
                    if let GgrsEvent::DesyncDetected { frame, .. } = event {
                        panic!("player {me} saw a desync at tick {frame}");
                    }
                }
                if session.current_state() != SessionState::Running {
                    continue;
                }
                let tick = session.current_frame().max(0) as u32;
                session.add_local_input(me, input(me, tick)).unwrap();
                let requests = match session.advance_frame() {
                    Ok(requests) => requests,
                    Err(GgrsError::PredictionThreshold) => continue,
                    Err(e) => panic!("player {me}: {e}"),
                };
                let mut stepped = 0;
                for request in requests {
                    if let GgrsRequest::SaveGameState { frame, .. } = &request {
                        sums.insert(*frame, sim.checksum());
                    }
                    stepped += fulfil(sim, vec![request]);
                }
                *rollbacks += (stepped > 1) as u32;
            }
            std::thread::sleep(Duration::from_micros(1_000_000 / TICK_HZ as u64));
        }
        let confirmed = peers.iter().map(|p| p.0.confirmed_frame()).min().unwrap();
        for frame in 0..=confirmed {
            assert_eq!(peers[0].2.get(&frame), peers[1].2.get(&frame), "tick {frame} differs");
        }
        assert!(peers.iter().any(|p| p.3 > 0), "late inputs should have rolled back");
    }
}
