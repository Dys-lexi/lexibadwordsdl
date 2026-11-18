//! Character selections at completed game ticks, independent of chat activity.

#[derive(Clone, Debug)]
pub(super) struct PlayerHero {
    pub account_id: Option<u32>,
    pub player_slot: Option<u32>,
    pub class_id: u32,
}

#[derive(Clone, Debug)]
pub(super) struct HeroSnapshot {
    pub game_time: f64,
    pub players: Vec<PlayerHero>,
}

impl HeroSnapshot {
    pub fn class_id(&self, account_id: Option<u32>, slot: Option<u32>) -> Option<u32> {
        if let Some(id) = account_id.filter(|id| *id != 0)
            && let Some(player) = self
                .players
                .iter()
                .find(|player| player.account_id == Some(id))
        {
            return Some(player.class_id);
        }
        // Slot fallback is safe only when it doesn't identify a different account.
        self.players
            .iter()
            .find(|player| {
                slot.is_some()
                    && player.player_slot == slot
                    && (account_id.is_none()
                        || player.account_id.is_none()
                        || player.account_id == account_id)
            })
            .map(|player| player.class_id)
    }
}

#[derive(Default)]
pub(super) struct HeroSnapshots {
    start: Option<HeroSnapshot>,
    minute: Option<HeroSnapshot>,
}

impl HeroSnapshots {
    pub fn complete(&self) -> bool {
        self.start.is_some() && self.minute.is_some()
    }

    pub fn needs(&self, time: f64) -> bool {
        time.is_finite()
            && time >= 0.0
            && (self.start.is_none() || (time >= 60.0 && self.minute.is_none()))
    }

    pub fn observe(&mut self, snapshot: HeroSnapshot) {
        if !self.needs(snapshot.game_time) || snapshot.players.is_empty() {
            return;
        }
        if self.start.is_none() {
            self.start = Some(snapshot.clone());
        }
        if snapshot.game_time >= 60.0 && self.minute.is_none() {
            self.minute = Some(snapshot);
        }
    }

    pub fn selected(&self, duration_s: Option<u32>) -> Option<&HeroSnapshot> {
        match duration_s {
            Some(duration) if duration < 60 => self.start.as_ref(),
            Some(_) => self.minute.as_ref(),
            None => self.minute.as_ref().or(self.start.as_ref()),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn snapshot(time: f64, class_id: u32) -> HeroSnapshot {
        HeroSnapshot {
            game_time: time,
            players: vec![PlayerHero {
                account_id: Some(123),
                player_slot: Some(2),
                class_id,
            }],
        }
    }

    #[test]
    fn samples_start_and_minute_ignoring_later_character_changes() {
        let mut samples = HeroSnapshots::default();
        for (time, hero) in [
            (-1.0, 1),
            (0.0, 2),
            (30.0, 3),
            (59.99, 4),
            (60.0, 5),
            (90.0, 6),
        ] {
            samples.observe(snapshot(time, hero));
        }
        assert_eq!(
            samples
                .selected(Some(120))
                .unwrap()
                .class_id(Some(123), None),
            Some(5)
        );
        assert_eq!(samples.selected(Some(60)).unwrap().game_time, 60.0);
        assert_eq!(
            samples
                .selected(Some(59))
                .unwrap()
                .class_id(Some(123), None),
            Some(2)
        );
    }

    #[test]
    fn short_match_uses_start_and_missing_minute_stays_unknown() {
        let mut samples = HeroSnapshots::default();
        samples.observe(snapshot(0.015625, 2));
        samples.observe(snapshot(25.0, 3));
        assert_eq!(
            samples
                .selected(Some(30))
                .unwrap()
                .class_id(Some(123), None),
            Some(2)
        );
        assert!(samples.selected(Some(120)).is_none());
        // First recorded tick after the target, when no exact 60-second tick exists.
        samples.observe(snapshot(60.015625, 4));
        assert_eq!(samples.selected(Some(120)).unwrap().game_time, 60.015625);
    }

    #[test]
    fn identities_prefer_account_and_do_not_confuse_reused_slots() {
        let sample = snapshot(60.0, 5);
        assert_eq!(sample.class_id(Some(123), Some(99)), Some(5));
        assert_eq!(sample.class_id(None, Some(2)), Some(5));
        assert_eq!(sample.class_id(Some(456), Some(2)), None);
        assert_eq!(sample.class_id(None, None), None);
    }
}
