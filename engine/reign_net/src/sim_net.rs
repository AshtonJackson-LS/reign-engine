//! An in-process network with latency, jitter and packet loss, for testing
//! sessions without UDP sockets.
//!
//! ggrs keeps time with the real clock, so packets are held for real
//! durations. Which packets are lost and how much jitter each gets come
//! from a seeded RNG, so a run loses the same packets every time. Jitter
//! reorders packets, as a real network does.

use std::net::{Ipv4Addr, SocketAddr};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use reign_core::Rng;

use crate::{Message, NonBlockingSocket};

/// How the network treats every packet.
#[derive(Clone, Copy, Debug)]
pub struct Conditions {
    /// One-way delay.
    pub latency: Duration,
    /// Up to this much extra delay, picked per packet.
    pub jitter: Duration,
    /// Chance in percent that a packet is lost.
    pub loss_percent: u32,
}

impl Conditions {
    pub const PERFECT: Conditions =
        Conditions { latency: Duration::ZERO, jitter: Duration::ZERO, loss_percent: 0 };
}

struct InFlight {
    due: Instant,
    from: SocketAddr,
    to: SocketAddr,
    message: Message,
}

struct Shared {
    conditions: Conditions,
    rng: Rng,
    in_flight: Vec<InFlight>,
}

/// The network. Clones share it.
#[derive(Clone)]
pub struct SimNetwork(Arc<Mutex<Shared>>);

impl SimNetwork {
    pub fn new(conditions: Conditions, seed: u64) -> Self {
        SimNetwork(Arc::new(Mutex::new(Shared {
            conditions,
            rng: Rng::new(seed),
            in_flight: Vec::new(),
        })))
    }

    /// The made-up address of endpoint `n`, for [`crate::Seat::Remote`].
    pub fn addr(n: u16) -> SocketAddr {
        SocketAddr::from((Ipv4Addr::new(10, 0, 0, 1), n))
    }

    /// A socket on this network at [`SimNetwork::addr`]`(n)`.
    pub fn socket(&self, n: u16) -> SimSocket {
        SimSocket { network: self.clone(), addr: Self::addr(n) }
    }

    pub fn set_conditions(&self, conditions: Conditions) {
        self.0.lock().unwrap().conditions = conditions;
    }
}

pub struct SimSocket {
    network: SimNetwork,
    addr: SocketAddr,
}

impl NonBlockingSocket<SocketAddr> for SimSocket {
    fn send_to(&mut self, msg: &Message, addr: &SocketAddr) {
        let mut net = self.network.0.lock().unwrap();
        let c = net.conditions;
        if net.rng.range(0, 99) < c.loss_percent as i32 {
            return;
        }
        let jitter = Duration::from_micros(net.rng.range(0, c.jitter.as_micros() as i32) as u64);
        let due = Instant::now() + c.latency + jitter;
        net.in_flight.push(InFlight { due, from: self.addr, to: *addr, message: msg.clone() });
    }

    fn receive_all_messages(&mut self) -> Vec<(SocketAddr, Message)> {
        let now = Instant::now();
        let mut net = self.network.0.lock().unwrap();
        let mut arrived: Vec<InFlight> = Vec::new();
        let mut i = 0;
        while i < net.in_flight.len() {
            let p = &net.in_flight[i];
            if p.to == self.addr && p.due <= now {
                arrived.push(net.in_flight.swap_remove(i));
            } else {
                i += 1;
            }
        }
        // In the order they landed, which jitter may have shuffled.
        arrived.sort_by_key(|p| p.due);
        arrived.into_iter().map(|p| (p.from, p.message)).collect()
    }
}
