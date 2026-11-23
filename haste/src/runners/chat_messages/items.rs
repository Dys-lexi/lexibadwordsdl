//! Shop notifications identify transactions. Item entities identify the owner
//! and imbue targets. Both messages can occur in either order within a tick.

use super::{
    CONTROLLER, Context, Entity, PAWN, PAWN_CONTROLLER, PLAYER_ASSISTS, PLAYER_DEATHS,
    PLAYER_KILLS, PLAYER_SOULS, PLAYER_STEAM_ID, account_id, game_time, info_account_id, user_info,
};
use crate::entities::{DeltaHeader, ehandle_to_index, fkey_from_path, is_ehandle_valid};
use crate::fxhash;
use crate::valveprotos::deadlock::CCitadelUserMessageItemPurchaseNotification;
use serde_json::{Value, json};

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
    pub(super) rows: Vec<Value>,
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
            self.rows.push(transaction.finish(ctx, &self.changes));
        }
        self.changes.clear();
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

impl Transaction {
    fn finish(self, ctx: &Context, changes: &[ItemChange]) -> Value {
        let mut candidates = changes.iter().filter(|c| {
            Some(c.item) == self.item
                && c.bought == self.bought
                && self.buyer.is_none_or(|id| id == c.owner)
        });
        let instance = candidates.next().filter(|_| candidates.next().is_none());
        let buyer = self.buyer.or_else(|| instance.map(|i| i.owner));
        let controller = buyer.and_then(|id| {
            ctx.entities()?
                .iter()
                .map(|(_, e)| e)
                .find(|e| e.serializer_name_heq(CONTROLLER) && controller_account(e) == Some(id))
        });
        let stat = |key| controller.and_then(|e| e.get_value::<i32>(key));
        json!({
            "account_id": buyer,
            "tick": self.tick,
            "game_time": self.game_time,
            "imbuedability": instance.map(|i| i.imbued_abilities.as_slice()).unwrap_or_default(),
            "item": self.item,
            "kills": stat(&PLAYER_KILLS),
            "deaths": stat(&PLAYER_DEATHS),
            "assists": stat(&PLAYER_ASSISTS),
            "souls": stat(&PLAYER_SOULS),
            "bought": self.bought,
        })
    }
}
