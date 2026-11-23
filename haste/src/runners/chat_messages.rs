mod items;

use std::collections::BTreeMap;
use std::io::{self, Read, Seek};
use std::sync::Arc;

use super::hero_snapshots::{HeroSnapshot, HeroSnapshots, PlayerHero};
use super::match_details::{MatchInfo, MatchMetadata};
use crate::demofile::DemoFile;
use crate::demostream::CmdHeader;
use crate::entities::{DeltaHeader, Entity, ehandle_to_index, fkey_from_path, is_ehandle_valid};
use crate::fieldvalue::FieldValue;
use crate::fxhash;
use crate::parser::{Context, Parser, Visitor};
use crate::valveprotos::common::{CDemoClassInfo, CDemoFileHeader, CMsgPlayerInfo, EDemoCommands};
use crate::valveprotos::deadlock::{
    CCitadelUserMessageItemPurchaseNotification, CCitadelUserMsgChatMsg,
    CCitadelUserMsgPostMatchDetails, CitadelUserMessageIds,
};
use base64::Engine as _;
use prost::Message as _;
use serde_json::{Value, json};

const CONTROLLER: u64 = fxhash::hash_bytes(b"CCitadelPlayerController");
const PAWN: u64 = fxhash::hash_bytes(b"CCitadelPlayerPawn");
const RULES: u64 = fxhash::hash_bytes(b"CCitadelGameRulesProxy");
const PLAYER_SLOT: u64 = fkey_from_path(&["m_unLobbyPlayerSlot"]);
const PLAYER_STEAM_ID: u64 = fkey_from_path(&["m_steamID"]);
const PLAYER_NAME: u64 = fkey_from_path(&["m_iszPlayerName"]);
const PLAYER_TEAM: u64 = fkey_from_path(&["m_iTeamNum"]);
const PLAYER_HERO: u64 = fkey_from_path(&["m_PlayerDataGlobal", "m_nHeroID"]);
const PLAYER_SOULS: u64 = fkey_from_path(&["m_PlayerDataGlobal", "m_iGoldNetWorth"]);
const PLAYER_KILLS: u64 = fkey_from_path(&["m_PlayerDataGlobal", "m_iPlayerKills"]);
const PLAYER_DEATHS: u64 = fkey_from_path(&["m_PlayerDataGlobal", "m_iDeaths"]);
const PLAYER_ASSISTS: u64 = fkey_from_path(&["m_PlayerDataGlobal", "m_iPlayerAssists"]);
const PLAYER_PAWN: u64 = fkey_from_path(&["m_hPawn"]);
const PAWN_CONTROLLER: u64 = fkey_from_path(&["m_hController"]);
const PAWN_BUILD: u64 = fkey_from_path(&["m_sHeroBuildSerialized"]);
const PAWN_BUILD_ID: u64 = fkey_from_path(&["m_unHeroBuildID"]);
const GAME_START: u64 = fkey_from_path(&["m_pGameRules", "m_flGameStartTime"]);
const PAUSED_TICKS: u64 = fkey_from_path(&["m_pGameRules", "m_nTotalPausedTicks"]);

// Steam64 IDs encode the 32-bit account ID above this base.
const STEAM64_BASE: u64 = 76_561_197_960_265_728;

pub(super) fn account_id(steam_id: u64) -> Option<u32> {
    if steam_id >= STEAM64_BASE {
        u32::try_from(steam_id - STEAM64_BASE).ok()
    } else {
        u32::try_from(steam_id).ok()
    }
}

fn user_info(ctx: &Context, slot: i32) -> Option<CMsgPlayerInfo> {
    let table = ctx.string_tables()?.find_table("userinfo")?;
    let bytes = table.get_item(&slot)?.get_user_data()?;
    CMsgPlayerInfo::decode(bytes).ok()
}

fn info_account_id(info: &CMsgPlayerInfo) -> Option<u32> {
    info.steamid
        .or(info.xuid)
        .and_then(account_id)
        .filter(|id| *id != 0)
}

pub(super) fn game_time(ctx: &Context) -> Option<f64> {
    game_time_at(ctx, ctx.tick())
}

fn game_time_at(ctx: &Context, tick: i32) -> Option<f64> {
    let entities = ctx.entities()?;
    let rules = entities
        .iter()
        .map(|(_, entity)| entity)
        .find(|entity| entity.serializer_name_heq(RULES))?;
    let start: f32 = rules.get_value(&GAME_START)?;
    let paused: i32 = rules.get_value(&PAUSED_TICKS).unwrap_or_default();
    let interval = ctx.tick_interval();
    (interval > 0.0)
        .then(|| f64::from(tick.saturating_sub(paused)) * f64::from(interval) - f64::from(start))
}

fn hero_snapshot(ctx: &Context, tick: i32, samples: &HeroSnapshots) -> Option<HeroSnapshot> {
    if samples.complete() {
        return None;
    }
    let entities = ctx.entities()?;
    // Before the game starts this field is zero, not a valid clock origin.
    let rules = entities
        .iter()
        .map(|(_, entity)| entity)
        .find(|entity| entity.serializer_name_heq(RULES))?;
    if rules.get_value::<f32>(&GAME_START)? <= 0.0 {
        return None;
    }
    let time = game_time_at(ctx, tick)?;
    if !samples.needs(time) {
        return None;
    }
    let players = entities
        .iter()
        .filter_map(|(_, entity)| {
            if !entity.serializer_name_heq(CONTROLLER) {
                return None;
            }
            let class_id = entity.get_value::<u32>(&PLAYER_HERO).filter(|id| *id > 0)?;
            let slot = entity.get_value::<u8>(&PLAYER_SLOT).map(u32::from);
            let id = entity
                .get_value::<u64>(&PLAYER_STEAM_ID)
                .and_then(account_id)
                .or_else(|| {
                    let table = ctx.string_tables()?.find_table("userinfo")?;
                    let bytes = table.get_item(&(slot? as i32))?.get_user_data()?;
                    CMsgPlayerInfo::decode(bytes)
                        .ok()?
                        .steamid
                        .and_then(account_id)
                });
            Some(PlayerHero {
                account_id: id,
                player_slot: slot,
                class_id,
            })
        })
        .collect();
    Some(HeroSnapshot {
        game_time: time,
        players,
    })
}

#[derive(Default)]
struct ChatVisitor {
    messages: Vec<Value>,
    items: items::Transactions,
    player_names: BTreeMap<u32, String>,
    events: Vec<Value>,
    entity_names: BTreeMap<u64, String>,
    heroes: HeroSnapshots,
    last_tick: Option<i32>,
    builds_by_controller: BTreeMap<i32, PlayerBuild>,
    hero_classes: BTreeMap<u32, String>,
    game_build: Option<i32>,
    map: Option<String>,
    match_info: Option<MatchInfo>,
    match_start_unix: Option<f64>,
}

#[derive(Default)]
struct PlayerBuild {
    raw_bytes: Option<Arc<[u8]>>,
    id: Option<u64>,
}

impl ChatVisitor {
    fn remember_names(ctx: &Context, names: &mut BTreeMap<u32, String>) {
        if let Some(table) = ctx
            .string_tables()
            .and_then(|tables| tables.find_table("userinfo"))
        {
            for (_, item) in table.items() {
                if let Some(bytes) = item.get_user_data()
                    && let Ok(info) = CMsgPlayerInfo::decode(bytes)
                    && let Some(id) = info.steamid.or(info.xuid).and_then(account_id)
                    && let Some(name) = info.name.filter(|name| !name.is_empty())
                {
                    names.insert(id, name);
                }
            }
        }
    }

    fn record(&mut self, ctx: &Context, chat: CCitadelUserMsgChatMsg) {
        let slot = chat.player_slot;
        let user_info = slot.and_then(|slot| user_info(ctx, slot));
        let user_account_id = user_info.as_ref().and_then(info_account_id);

        let entities = ctx.entities();
        let controller = entities.and_then(|entities| {
            let sender = user_account_id?;
            // Chat slots index userinfo, not m_unLobbyPlayerSlot. Joining those
            // slots can select another player's stats (or no controller at all).
            entities.iter().map(|(_, entity)| entity).find(|entity| {
                entity.serializer_name_heq(CONTROLLER)
                    && entity
                        .get_value::<u64>(&PLAYER_STEAM_ID)
                        .and_then(account_id)
                        == Some(sender)
            })
        });
        let account_id = user_account_id;
        let team = controller.and_then(|entity| entity.get_value::<u8>(&PLAYER_TEAM));
        let class_id = controller.and_then(|entity| entity.get_value::<u32>(&PLAYER_HERO));
        let class = class_id.and_then(|id| self.hero_classes.get(&id));
        let pawn = controller
            .and_then(|entity| entity.get_value::<u32>(&PLAYER_PAWN))
            .filter(|handle| is_ehandle_valid(*handle))
            .and_then(|handle| entities?.get(&ehandle_to_index(handle)))
            .filter(|entity| entity.serializer_name_heq(PAWN));

        // Team souls are the sum of each player's net worth at this tick.
        let mut team_souls = BTreeMap::<u8, i64>::new();
        let mut team_kills = BTreeMap::<u8, i64>::new();
        if let Some(entities) = entities {
            for (_, entity) in entities.iter() {
                if entity.serializer_name_heq(CONTROLLER)
                    && let Some(team) = entity.get_value::<u8>(&PLAYER_TEAM)
                {
                    let kills = entity.get_value::<i32>(&PLAYER_KILLS).unwrap_or_default();
                    *team_kills.entry(team).or_default() += i64::from(kills);
                    if let Some(souls) = entity.get_value::<i32>(&PLAYER_SOULS) {
                        *team_souls.entry(team).or_default() += i64::from(souls);
                    }
                }
            }
        }
        let enemy_team = team.and_then(|team| {
            team_souls
                .keys()
                .chain(team_kills.keys())
                .copied()
                .find(|other| *other != team && *other >= 2)
        });
        let game_time = game_time(ctx);
        let current_build = pawn.and_then(|entity| match entity.get_field_value(&PAWN_BUILD) {
            Some(FieldValue::String(bytes)) => Some(bytes.as_ref()),
            _ => None,
        });
        let cached_build =
            controller.and_then(|entity| self.builds_by_controller.get(&entity.index()));
        let build = current_build
            .or_else(|| cached_build.and_then(|build| build.raw_bytes.as_deref()))
            .map(|bytes| base64::engine::general_purpose::STANDARD.encode(bytes));
        let build_id = pawn
            .and_then(|entity| entity.get_value::<u64>(&PAWN_BUILD_ID))
            .or_else(|| cached_build.and_then(|build| build.id));

        self.messages.push(json!({
            "tick": ctx.tick(),
            "text": chat.text,
            "all_chat": chat.all_chat,
            "player_slot": slot,
            "account_id": account_id,
            "player_name": user_info.and_then(|info| info.name),
            "unix": self.match_start_unix,
            "game_time": game_time,
            "class": class,
            "class_id": class_id,
            "souls": controller.and_then(|entity| entity.get_value::<i32>(&PLAYER_SOULS)),
            "team": team,
            "kills": controller.and_then(|entity| entity.get_value::<i32>(&PLAYER_KILLS)),
            "deaths": controller.and_then(|entity| entity.get_value::<i32>(&PLAYER_DEATHS)),
            "assists": controller.and_then(|entity| entity.get_value::<i32>(&PLAYER_ASSISTS)),
            "team_souls": team.and_then(|team| team_souls.get(&team).copied()),
            "enemy_team_souls": enemy_team.and_then(|team| team_souls.get(&team).copied()),
            "team_kills": team.and_then(|team| team_kills.get(&team).copied()),
            "enemy_team_kills": enemy_team.and_then(|team| team_kills.get(&team).copied()),
            "build": build,
            "build_id": build_id,
            "game_build": self.game_build,
        }));
    }
}

impl Visitor for ChatVisitor {
    type Error = io::Error;

    fn on_entity(
        &mut self,
        ctx: &Context,
        delta_header: DeltaHeader,
        entity: &Entity,
    ) -> Result<(), Self::Error> {
        self.items.observe(ctx, delta_header, entity);
        if entity.serializer_name_heq(CONTROLLER)
            && let Some(id) = entity
                .get_value::<u64>(&PLAYER_STEAM_ID)
                .and_then(account_id)
            && let Some(name) = entity
                .get_value::<String>(&PLAYER_NAME)
                .filter(|name| !name.is_empty())
        {
            self.player_names.insert(id, name);
        }
        if entity.serializer_name_heq(PAWN)
            && let Some(handle) = entity.get_value::<u32>(&PAWN_CONTROLLER)
            && is_ehandle_valid(handle)
        {
            let build = self
                .builds_by_controller
                .entry(ehandle_to_index(handle))
                .or_default();
            if let Some(FieldValue::String(bytes)) = entity.get_field_value(&PAWN_BUILD) {
                build.raw_bytes = Some(Arc::clone(bytes));
            }
            if let Some(id) = entity.get_value::<u64>(&PAWN_BUILD_ID) {
                build.id = Some(id);
            }
        }
        Ok(())
    }

    fn on_cmd(
        &mut self,
        ctx: &Context,
        header: &CmdHeader,
        data: &[u8],
    ) -> Result<(), Self::Error> {
        // on_tick_end runs after the first command of a tick, which can omit
        // subsequent entity updates. Sample the completed previous tick here.
        if self.last_tick != Some(header.tick) {
            self.items.finish_tick(ctx);
        }
        if let Some(tick) = self.last_tick
            && tick != header.tick
            && let Some(snapshot) = hero_snapshot(ctx, tick, &self.heroes)
        {
            self.heroes.observe(snapshot);
        }
        self.last_tick = Some(header.tick);
        if matches!(
            header.cmd,
            EDemoCommands::DemFullPacket | EDemoCommands::DemStop
        ) {
            Self::remember_names(ctx, &mut self.player_names);
        }
        if header.cmd == EDemoCommands::DemFileHeader {
            let header = CDemoFileHeader::decode(data).map_err(io::Error::other)?;
            self.game_build = header.build_num;
            self.map = header.map_name;
        } else if header.cmd == EDemoCommands::DemClassInfo {
            let classes = CDemoClassInfo::decode(data).map_err(io::Error::other)?;
            for class in classes.classes {
                if let Some(name) = class.network_name {
                    self.entity_names
                        .insert(fxhash::hash_bytes(name.as_bytes()), name);
                }
            }
        }
        Ok(())
    }

    fn on_packet(
        &mut self,
        ctx: &Context,
        packet_type: u32,
        data: &[u8],
    ) -> Result<(), Self::Error> {
        if let Some(event) = super::death_events::record(
            ctx,
            packet_type,
            data,
            &self.hero_classes,
            &self.entity_names,
        )? {
            self.events.push(event);
        }
        if packet_type == CitadelUserMessageIds::KEUserMsgChatMsg as u32 {
            self.record(
                ctx,
                CCitadelUserMsgChatMsg::decode(data).map_err(io::Error::other)?,
            );
        } else if packet_type == CitadelUserMessageIds::KEUserMsgItemPurchaseNotification as u32 {
            let notification = CCitadelUserMessageItemPurchaseNotification::decode(data)
                .map_err(io::Error::other)?;
            self.items.record(ctx, notification);
        } else if packet_type == CitadelUserMessageIds::KEUserMsgPostMatchDetails as u32 {
            let details =
                CCitadelUserMsgPostMatchDetails::decode(data).map_err(io::Error::other)?;
            if let Some(bytes) = details.match_details {
                let metadata = MatchMetadata::decode(bytes.as_slice()).map_err(io::Error::other)?;
                if let Some(info) = metadata.match_info {
                    self.match_info = Some(info);
                }
            }
        }
        Ok(())
    }
}

/// Extract match details, chat, death events, and shop transactions from a demo.
pub fn run<R: Read + Seek>(
    reader: R,
    match_start_unix: Option<f64>,
) -> Result<Value, super::RunError> {
    let demo_file = DemoFile::start_reading(reader)?;
    let hero_classes = serde_json::from_str(include_str!("../../data/hero-classes.json"))
        .map_err(io::Error::other)?;
    let mut parser = Parser::from_stream_with_visitor(
        demo_file,
        ChatVisitor {
            match_start_unix,
            hero_classes,
            ..ChatVisitor::default()
        },
    )?;
    parser.run_to_end()?;
    // Cache the final userinfo state, including demos ending without DEM_Stop.
    let mut names = BTreeMap::new();
    ChatVisitor::remember_names(parser.context(), &mut names);
    parser.visitor_mut().player_names.extend(names);
    let mut heroes = std::mem::take(&mut parser.visitor_mut().heroes);
    if let Some(snapshot) = hero_snapshot(parser.context(), parser.context().tick(), &heroes) {
        heroes.observe(snapshot);
    }
    let mut items = std::mem::take(&mut parser.visitor_mut().items);
    items.finish_tick(parser.context());
    let mut visitor = parser.into_visitor();
    // PostMatchDetails arrives after the chats, so fill earlier messages too.
    let unix = visitor.match_start_unix.or_else(|| {
        visitor
            .match_info
            .as_ref()
            .and_then(|info| info.start_time)
            .filter(|start| *start > 0)
            .map(f64::from)
    });
    let timestamp = json!(unix);
    for message in &mut visitor.messages {
        message["unix"] = timestamp.clone();
    }
    let present = visitor.match_info.is_some();
    let info = visitor.match_info.take().unwrap_or_default();
    let mut summary = info.summary(unix, visitor.game_build, visitor.map.as_deref(), present);
    let characters = heroes.selected(info.duration_s);
    summary["character_game_time"] = json!(characters.map(|snapshot| snapshot.game_time));
    if let Some(players) = summary["players"].as_array_mut() {
        for (player_json, player) in players.iter_mut().zip(&info.players) {
            player_json["personaname"] = json!(
                player
                    .account_id
                    .and_then(|id| visitor.player_names.get(&id))
            );
            let class_id = characters
                .and_then(|snapshot| snapshot.class_id(player.account_id, player.player_slot));
            player_json["class_id"] = json!(class_id);
            player_json["class"] = json!(class_id.and_then(|id| visitor.hero_classes.get(&id)));
        }
    }
    Ok(json!({
        "match": summary,
        "chat": std::mem::take(&mut visitor.messages),
        "events": std::mem::take(&mut visitor.events),
        "items": items.rows(&info),
    }))
}
