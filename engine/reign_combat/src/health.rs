//! Shared damage and invulnerability rules for players and enemies.

#[derive(
    Clone,
    Copy,
    Debug,
    Default,
    Hash,
    PartialEq,
    Eq,
    serde::Serialize,
    serde::Deserialize,
    reign_scene::Reflect,
)]
pub struct Health {
    pub current: i32,
    pub max: i32,
    pub invulnerable: u16,
}

impl Health {
    pub fn new(max: i32) -> Self {
        assert!(max > 0);
        Self { current: max, max, invulnerable: 0 }
    }

    pub fn alive(&self) -> bool {
        self.current > 0
    }

    pub fn step(&mut self) {
        self.invulnerable = self.invulnerable.saturating_sub(1);
    }

    /// Returns true only when damage was accepted. Dead actors cannot be hit.
    pub fn damage(&mut self, amount: i32, invulnerability: u16) -> bool {
        if amount <= 0 || !self.alive() || self.invulnerable > 0 {
            return false;
        }
        self.current = self.current.saturating_sub(amount).max(0);
        self.invulnerable = invulnerability;
        true
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn invulnerability_expires_and_death_is_final() {
        let mut health = Health::new(3);
        assert!(health.damage(1, 2));
        assert!(!health.damage(1, 2));
        health.step();
        assert!(!health.damage(1, 2));
        health.step();
        assert!(health.damage(10, 0));
        assert_eq!(health.current, 0);
        assert!(!health.damage(1, 0));
        assert!(!health.alive());
    }
}
