//! Shop notifications identify transactions. Item entities identify the owner
//! and imbue targets. Both messages can occur in either order within a tick.

use super::super::match_details::MatchInfo;
use super::{
    CONTROLLER, Context, Entity, PAWN, PAWN_CONTROLLER, PLAYER_ASSISTS, PLAYER_DEATHS,
    PLAYER_KILLS, PLAYER_SOULS, PLAYER_STEAM_ID, account_id, game_time, info_account_id, user_info,
};
use crate::entities::{DeltaHeader, ehandle_to_index, fkey_from_path, is_ehandle_valid};
use crate::fxhash;
use crate::valveprotos::deadlock::CCitadelUserMessageItemPurchaseNotification;
use serde_json::{Value, json};
use std::collections::BTreeMap;
use std::ops::Range;

const ITEM: u64 = fkey_from_path(&["m_nSubclassID"]);
const OWNER: u64 = fkey_from_path(&["m_hOwnerEntity"]);
const IMBUES: u64 = fkey_from_path(&["m_vecImbuedAbilities"]);

struct ItemChange {
    item: u32,
    bought: bool,
    owner: u32,
    imbued_abilities: Vec<u32>,
}

fn controller_account(entity: &Entity) -> Option<u32> {
    account_id(entity.get_value::<u64>(&PLAYER_STEAM_ID)?).filter(|id| *id != 0)
}

#[derive(Default)]
pub(super) struct Transactions {
    pending: Vec<Transaction>,
    changes: Vec<ItemChange>,
    recorded: Vec<Recorded>,
}

impl Transactions {
    pub(super) fn observe(&mut self, ctx: &Context, delta: DeltaHeader, entity: &Entity) {
        let bought = match delta {
            DeltaHeader::CREATE => true,
            DeltaHeader::DELETE => false,
            _ => return,
        };
        if let Some(change) = ItemChange::from_entity(ctx, entity, bought) {
            self.changes.push(change);
        }
    }

    pub(super) fn record(
        &mut self,
        ctx: &Context,
        msg: CCitadelUserMessageItemPurchaseNotification,
    ) {
        let buyer = msg
            .userid
            .and_then(|slot| user_info(ctx, slot))
            .as_ref()
            .and_then(info_account_id);
        self.pending.push(Transaction {
            buyer,
            tick: ctx.tick(),
            item: msg.ability_id,
            bought: !msg.sell(),
            game_time: game_time(ctx),
        });
    }

    pub(super) fn finish_tick(&mut self, ctx: &Context) {
        for transaction in self.pending.drain(..) {
            self.recorded.push(transaction.finish(ctx, &self.changes));
        }
        self.changes.clear();
    }

    pub(super) fn rows(&self, info: &MatchInfo) -> Vec<Value> {
        let mut owners: Vec<_> = self.recorded.iter().map(|r| r.transaction.buyer).collect();
        if owners.contains(&None) {
            self.resolve_history(info, &mut owners);
        }
        self.recorded
            .iter()
            .zip(owners)
            .map(|(r, owner)| r.json(owner))
            .collect()
    }

    fn resolve_history(&self, info: &MatchInfo, owners: &mut [Option<u32>]) {
        #[derive(Default)]
        struct Group {
            notifications: Vec<usize>,
            history: Vec<(Option<u32>, u32)>,
        }
        let mut groups = BTreeMap::<(u32, bool), Group>::new();
        for (index, row) in self.recorded.iter().enumerate() {
            let tx = &row.transaction;
            if let Some(item) = tx.item {
                groups
                    .entry((item, tx.bought))
                    .or_default()
                    .notifications
                    .push(index);
            }
        }
        for player in &info.players {
            for entry in &player.items {
                let Some(item) = entry.item_id else { continue };
                for (bought, time) in [
                    (true, entry.game_time_s),
                    (false, entry.sold_time_s.filter(|s| *s > 0)),
                ] {
                    if let (Some(second), Some(group)) = (time, groups.get_mut(&(item, bought))) {
                        group
                            .history
                            .push((player.account_id.filter(|id| *id != 0), second));
                    }
                }
            }
        }

        // Each independently identified action bounds the clock offset to a
        // one-second interval because post-match timestamps are truncated.
        let mut offset = f64::NEG_INFINITY..f64::INFINITY;
        for group in groups.values() {
            if let ([index], [(Some(account), second)]) =
                (group.notifications.as_slice(), group.history.as_slice())
            {
                let tx = &self.recorded[*index].transaction;
                if tx.buyer == Some(*account)
                    && let Some(time) = tx.game_time
                {
                    let delta = f64::from(*second) - time;
                    offset.start = offset.start.max(delta);
                    offset.end = offset.end.min(delta + 1.0);
                }
            }
        }
        if !offset.start.is_finite() || offset.is_empty() {
            return;
        }

        for group in groups.values() {
            for &index in &group.notifications {
                let tx = &self.recorded[index].transaction;
                if tx.buyer.is_some() {
                    continue;
                }
                let mut candidates = group
                    .history
                    .iter()
                    .filter(|(_, second)| tx.matches_time(*second, &offset));
                let Some(&(Some(account), second)) = candidates.next() else {
                    continue;
                };
                if candidates.next().is_some() {
                    continue;
                }
                // Refunded purchases may disappear from the history. Don't
                // attribute two nearby notifications to the same history entry.
                let competing = group.notifications.iter().any(|&other| {
                    let other_tx = &self.recorded[other].transaction;
                    other != index
                        && other_tx.buyer.is_none_or(|id| id == account)
                        && other_tx.matches_time(second, &offset)
                });
                if !competing {
                    owners[index] = Some(account);
                }
            }
        }
    }
}

impl ItemChange {
    fn from_entity(ctx: &Context, entity: &Entity, bought: bool) -> Option<Self> {
        let item = entity.get_value(&ITEM)?;
        // Shop upgrades have the imbue vector even when it is empty.
        let count = entity.get_value::<u32>(&IMBUES)?;
        let entities = ctx.entities()?;
        let resolve = |handle: u32| {
            if is_ehandle_valid(handle) {
                entities.get(&ehandle_to_index(handle))
            } else {
                None
            }
        };
        let pawn = resolve(entity.get_value(&OWNER)?)?;
        if !pawn.serializer_name_heq(PAWN) {
            return None;
        }
        // While a dead player is spectating, their hero pawn can have an
        // invalid m_hController but still belongs to their player controller.
        let owner = [PAWN_CONTROLLER, OWNER].into_iter().find_map(|field| {
            let controller = resolve(pawn.get_value(&field)?)?;
            if controller.serializer_name_heq(CONTROLLER) {
                controller_account(controller)
            } else {
                None
            }
        })?;
        // Entity field paths store each vector index as a byte.
        let imbued_abilities = (0..=u8::MAX)
            .take(count as usize)
            .filter_map(|index| {
                let key =
                    fxhash::add_u64_to_hash(IMBUES, fxhash::add_u64_to_hash(0, u64::from(index)));
                entity.get_value::<u32>(&key).filter(|id| *id != 0)
            })
            .collect();
        Some(Self {
            item,
            bought,
            owner,
            imbued_abilities,
        })
    }
}

struct Transaction {
    buyer: Option<u32>,
    tick: i32,
    item: Option<u32>,
    bought: bool,
    game_time: Option<f64>,
}

#[derive(Default)]
struct Stats {
    kills: Option<i32>,
    deaths: Option<i32>,
    assists: Option<i32>,
    souls: Option<i32>,
}

struct Recorded {
    transaction: Transaction,
    imbued_abilities: Vec<u32>,
    stats: Vec<(u32, Stats)>,
}

impl Transaction {
    fn matches_time(&self, second: u32, offset: &Range<f64>) -> bool {
        self.game_time.is_some_and(|time| {
            let delta = f64::from(second) - time;
            delta < offset.end && delta + 1.0 > offset.start
        })
    }

    fn finish(mut self, ctx: &Context, changes: &[ItemChange]) -> Recorded {
        let mut candidates = changes.iter().filter(|c| {
            Some(c.item) == self.item
                && c.bought == self.bought
                && self.buyer.is_none_or(|id| id == c.owner)
        });
        let instance = candidates.next().filter(|_| candidates.next().is_none());
        self.buyer = self.buyer.or_else(|| instance.map(|i| i.owner));
        // Anonymous notifications may only acquire an identity at the end of
        // the demo. Keep the candidate controllers' stats from this tick.
        let stats = ctx
            .entities()
            .into_iter()
            .flat_map(|entities| entities.iter())
            .filter(|(_, e)| e.serializer_name_heq(CONTROLLER))
            .filter_map(|(_, e)| {
                let id = controller_account(e)?;
                if self.buyer.is_some_and(|buyer| id != buyer) {
                    return None;
                }
                Some((
                    id,
                    Stats {
                        kills: e.get_value(&PLAYER_KILLS),
                        deaths: e.get_value(&PLAYER_DEATHS),
                        assists: e.get_value(&PLAYER_ASSISTS),
                        souls: e.get_value(&PLAYER_SOULS),
                    },
                ))
            })
            .collect();
        Recorded {
            transaction: self,
            imbued_abilities: instance
                .map(|i| i.imbued_abilities.clone())
                .unwrap_or_default(),
            stats,
        }
    }
}

impl Recorded {
    fn json(&self, buyer: Option<u32>) -> Value {
        let stats = self
            .stats
            .iter()
            .find(|(id, _)| Some(*id) == buyer)
            .map(|(_, s)| s);
        json!({
            "account_id": buyer,
            "tick": self.transaction.tick,
            "game_time": self.transaction.game_time,
            "imbuedability": self.imbued_abilities,
            "item": self.transaction.item,
            "kills": stats.and_then(|s| s.kills),
            "deaths": stats.and_then(|s| s.deaths),
            "assists": stats.and_then(|s| s.assists),
            "souls": stats.and_then(|s| s.souls),
            "bought": self.transaction.bought,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::runners::match_details::Player;
    use crate::valveprotos::deadlock::c_msg_match_meta_data_contents::Items;

    const COUNTERSPELL: u32 = 1_414_025_773;
    const BUYER: u32 = 1_169_121_188;

    fn row(bought: bool, time: f64) -> Recorded {
        Recorded {
            transaction: Transaction {
                item: Some(COUNTERSPELL),
                bought,
                tick: 74523,
                game_time: Some(time),
                buyer: None,
            },
            stats: vec![(
                BUYER,
                Stats {
                    kills: Some(5),
                    ..Default::default()
                },
            )],
            imbued_abilities: vec![],
        }
    }

    fn fixture() -> (Transactions, MatchInfo) {
        let mut anchor = row(true, 42.0);
        anchor.transaction.item = Some(123);
        anchor.transaction.buyer = Some(BUYER);
        let transactions = Transactions {
            recorded: vec![anchor, row(true, 1091.609375)],
            ..Default::default()
        };
        let info = MatchInfo {
            players: vec![Player {
                account_id: Some(BUYER),
                kills: Some(6),
                items: vec![
                    Items {
                        item_id: Some(123),
                        game_time_s: Some(100),
                        ..Default::default()
                    },
                    Items {
                        item_id: Some(COUNTERSPELL),
                        game_time_s: Some(1149),
                        sold_time_s: Some(1200),
                        ..Default::default()
                    },
                ],
                ..Default::default()
            }],
            ..Default::default()
        };
        (transactions, info)
    }

    #[test]
    fn history_resolves_anonymous_buys_and_sales_using_tick_stats() {
        let (mut transactions, info) = fixture();
        transactions
            .recorded
            .extend([row(false, 1142.0), row(false, 1091.609375)]);
        let rows = transactions.rows(&info);
        for r in &rows[1..3] {
            assert_eq!(r["account_id"], BUYER);
            assert_eq!(r["kills"], 5);
        }
        assert!(rows[3]["account_id"].is_null());
    }

    #[test]
    fn history_must_identify_one_player_and_one_notification() {
        let (mut transactions, mut info) = fixture();
        info.players.push(Player {
            account_id: Some(42),
            items: vec![info.players[0].items[1]],
            ..Default::default()
        });
        assert!(transactions.rows(&info)[1]["account_id"].is_null());
        info.players.pop();
        transactions.recorded.push(row(true, 1091.6));
        assert!(
            transactions.rows(&info)[1..]
                .iter()
                .all(|r| r["account_id"].is_null())
        );
    }
}
