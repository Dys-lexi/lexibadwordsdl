//! Kill-feed messages, resolved against entity state at the event's tick.

use std::collections::BTreeMap;
use std::io;

use crate::entities::{ehandle_to_index, fkey_from_path, is_ehandle_valid};
use crate::fxhash;
use crate::parser::Context;
use crate::valveprotos::common::CMsgPlayerInfo;
use crate::valveprotos::deadlock::{
    CCitadelUserMsgBossKilled, CCitadelUserMsgHeroKilled, CitadelUserMessageIds,
};
use prost::Message;
use serde_json::{Value, json};

use super::chat_messages::{account_id, game_time};

const CONTROLLER: u64 = fxhash::hash_bytes(b"CCitadelPlayerController");
const CONTROLLER_HANDLE: u64 = fkey_from_path(&["m_hController"]);
const SLOT: u64 = fkey_from_path(&["m_unLobbyPlayerSlot"]);
const STEAM_ID: u64 = fkey_from_path(&["m_steamID"]);
const TEAM: u64 = fkey_from_path(&["m_iTeamNum"]);
const HERO: u64 = fkey_from_path(&["m_PlayerDataGlobal", "m_nHeroID"]);
const PAWN_HERO: u64 = fkey_from_path(&["m_CCitadelHeroComponent", "m_spawnedHero", "m_nHeroID"]);

fn handle_index(handle: Option<u32>) -> Option<i32> {
    handle
        .filter(|h| is_ehandle_valid(*h) && *h != u32::MAX)
        .map(ehandle_to_index)
}

fn actor(
    ctx: &Context,
    index: Option<i32>,
    heroes: &BTreeMap<u32, String>,
    names: &BTreeMap<u64, String>,
) -> Value {
    let Some(index) = index.filter(|index| *index >= 0) else {
        return Value::Null;
    };
    let entities = ctx.entities();
    let entity = entities.and_then(|entities| entities.get(&index));
    let controller = entity.and_then(|entity| {
        if entity.serializer_name_heq(CONTROLLER) {
            Some(entity)
        } else {
            let index = handle_index(entity.get_value(&CONTROLLER_HANDLE))?;
            entities?
                .get(&index)
                .filter(|entity| entity.serializer_name_heq(CONTROLLER))
        }
    });
    let slot = controller.and_then(|entity| entity.get_value::<u8>(&SLOT));
    let uid = controller
        .and_then(|entity| entity.get_value::<u64>(&STEAM_ID))
        .and_then(account_id)
        .or_else(|| {
            let table = ctx.string_tables()?.find_table("userinfo")?;
            let bytes = table.get_item(&i32::from(slot?))?.get_user_data()?;
            CMsgPlayerInfo::decode(bytes)
                .ok()?
                .steamid
                .and_then(account_id)
        });
    let class_id = controller
        .and_then(|entity| entity.get_value::<u32>(&HERO))
        .or_else(|| entity.and_then(|entity| entity.get_value::<u32>(&PAWN_HERO)));
    let team = controller
        .or(entity)
        .and_then(|entity| entity.get_value::<u8>(&TEAM));
    json!({
        "entity_index": index,
        "entity_class": entity.and_then(|entity| names.get(&entity.serializer().serializer_name.hash)),
        "uid": uid,
        "account_id": uid,
        "player_slot": slot,
        "team": team,
        "class_id": class_id,
        "class": class_id.and_then(|id| heroes.get(&id)),
    })
}

pub(super) fn record(
    ctx: &Context,
    packet_type: u32,
    data: &[u8],
    heroes: &BTreeMap<u32, String>,
    names: &BTreeMap<u64, String>,
) -> Result<Option<Value>, io::Error> {
    let mut event = if packet_type == CitadelUserMessageIds::KEUserMsgHeroKilled as u32 {
        let killed = CCitadelUserMsgHeroKilled::decode(data).map_err(io::Error::other)?;
        let scorer = killed
            .entindex_scorer
            .filter(|index| *index >= 0)
            .or(killed.entindex_attacker);
        let assists: Vec<Value> = killed
            .entindex_assisters
            .iter()
            .map(|index| actor(ctx, Some(*index), heroes, names))
            .collect();
        json!({
            "type": "player_death",
            "tick": ctx.tick(),
            "game_time": game_time(ctx),
            "victim": actor(ctx, killed.entindex_victim, heroes, names),
            "killer": actor(ctx, scorer, heroes, names),
            "attacker": actor(ctx, killed.entindex_attacker, heroes, names),
            "assists": assists,
            "victim_team": killed.victim_team_number,
            "first_blood": killed.first_blood,
            "killer_ability_id": killed.killer_ability_id,
        })
    } else if packet_type == CitadelUserMessageIds::KEUserMsgBossKilled as u32 {
        let killed = CCitadelUserMsgBossKilled::decode(data).map_err(io::Error::other)?;
        let mut victim = actor(ctx, handle_index(killed.entity_killed), heroes, names);
        if let Some(fields) = victim.as_object_mut() {
            // Objective class codes share the output field with hero IDs, but
            // must not be looked up in the hero-name table.
            fields.insert("class_id".into(), json!(killed.entity_killed_class));
        }
        json!({
            "type": "objective_death",
            "tick": ctx.tick(),
            "game_time": game_time(ctx),
            "victim": victim,
            "killer": actor(ctx, handle_index(killed.entity_killer), heroes, names),
            // BossKilled has no assister field; null means unavailable, not zero assists.
            "assists": Value::Null,
            "objective_team": killed.objective_team,
            "objective_mask_change": killed.objective_mask_change,
            "bosses_remaining": killed.bosses_remaining,
            "killer_ability_id": killed.killer_ability_id,
            "server_game_time": killed.gametime,
        })
    } else {
        return Ok(None);
    };
    for prefix in ["victim", "killer"] {
        let actor = event
            .as_object_mut()
            .and_then(|event| event.remove(prefix))
            .unwrap_or(Value::Null);
        for field in [
            "account_id",
            "class",
            "class_id",
            "entity_class",
            "entity_index",
            "player_slot",
            "team",
            "uid",
        ] {
            let key = format!("{prefix}_{field}");
            // HeroKilled also reports victim_team directly. Preserve that value
            // when present, even if the victim entity cannot be resolved.
            if event.get(&key).is_none_or(Value::is_null) {
                event[key] = actor.get(field).cloned().unwrap_or(Value::Null);
            }
        }
    }
    Ok(Some(event))
}
