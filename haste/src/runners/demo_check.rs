//! The demo sanity check, expressed as JSON for the HTTP service.
use crate::demofile::DemoFile;
use crate::entities::{DeltaHeader, Entity, fkey_from_path};
use crate::fxhash;
use crate::parser::{Context, Parser, Visitor};
use crate::valveprotos::common::{CsvcMsgServerInfo, SvcMessages};
use prost::Message as _;
use serde_json::{Value, json};
use std::collections::BTreeMap;
use std::io::{self, Read, Seek};

const CELL_X: u64 = fkey_from_path(&["CBodyComponent", "m_cellX"]);
const CELL_Y: u64 = fkey_from_path(&["CBodyComponent", "m_cellY"]);
const CELL_Z: u64 = fkey_from_path(&["CBodyComponent", "m_cellZ"]);
const VEC_X: u64 = fkey_from_path(&["CBodyComponent", "m_vecX"]);
const VEC_Y: u64 = fkey_from_path(&["CBodyComponent", "m_vecY"]);
const VEC_Z: u64 = fkey_from_path(&["CBodyComponent", "m_vecZ"]);
const HEALTH: u64 = fkey_from_path(&["m_iHealth"]);
const MAX_HEALTH: u64 = fkey_from_path(&["m_iMaxHealth"]);
const TEAM: u64 = fkey_from_path(&["m_iTeamNum"]);
const LIFE_STATE: u64 = fkey_from_path(&["m_lifeState"]);
const HERO_ID: u64 = fkey_from_path(&["m_CCitadelHeroComponent", "m_spawnedHero", "m_nHeroID"]);
const PLAYER_NAME: u64 = fkey_from_path(&["m_iszPlayerName"]);
const NET_WORTH: u64 = fkey_from_path(&["m_PlayerDataGlobal", "m_iGoldNetWorth"]);

#[derive(Default)]
struct CheckVisitor {
    packets: BTreeMap<u32, (usize, usize)>,
    entity_updates: usize,
    encoding: Vec<Value>,
    samples: Vec<Value>,
}

impl Visitor for CheckVisitor {
    type Error = io::Error;

    fn on_packet(
        &mut self,
        _ctx: &Context,
        packet_type: u32,
        data: &[u8],
    ) -> Result<(), io::Error> {
        let entry = self.packets.entry(packet_type).or_default();
        entry.0 = entry.0.saturating_add(1);
        entry.1 = entry.1.saturating_add(data.len());
        if packet_type == SvcMessages::SvcServerInfo as u32 {
            let msg = CsvcMsgServerInfo::decode(data).map_err(io::Error::other)?;
            if let Some(config) = msg.game_session_config {
                self.encoding.push(json!({
                    "max_coord": config.max_coord,
                    "quantized_float_encoder_aliases": config.quantized_float_encoder_aliases
                        .into_iter().map(|alias| json!({"name": alias.name, "bit_count": alias.bit_count,
                            "min_value": alias.min_value, "max_value": alias.max_value,
                            "encode_flags": alias.encode_flags})).collect::<Vec<_>>()
                }));
            }
        }
        Ok(())
    }

    fn on_entity(
        &mut self,
        _ctx: &Context,
        _delta: DeltaHeader,
        _entity: &Entity,
    ) -> Result<(), io::Error> {
        self.entity_updates = self.entity_updates.saturating_add(1);
        Ok(())
    }

    fn on_tick_end(&mut self, ctx: &Context) -> Result<(), io::Error> {
        let interval = (300.0 / ctx.tick_interval()).round() as i32;
        if ctx.tick() <= 0 || interval <= 0 || ctx.tick().checked_rem(interval) != Some(0) {
            return Ok(());
        }
        let Some(entities) = ctx.entities() else {
            return Ok(());
        };
        let mut players = Vec::new();
        for (index, entity) in entities.iter() {
            if entity.serializer_name_heq(fxhash::hash_bytes(b"CCitadelPlayerController")) {
                players.push(json!({
                    "entity_index": index, "kind": "controller",
                    "name": entity.get_value::<String>(&PLAYER_NAME),
                    "net_worth": entity.get_value::<i32>(&NET_WORTH)
                }));
            } else if entity.serializer_name_heq(fxhash::hash_bytes(b"CCitadelPlayerPawn")) {
                let coord = |cell, vec| -> Option<f32> {
                    Some(ctx.deadlock_coord_from_cell(
                        entity.get_value(&cell)?,
                        entity.get_value(&vec)?,
                    ))
                };
                players.push(json!({
                    "entity_index": index, "kind": "pawn",
                    "position": [coord(CELL_X, VEC_X), coord(CELL_Y, VEC_Y), coord(CELL_Z, VEC_Z)],
                    "health": entity.get_value::<i32>(&HEALTH),
                    "max_health": entity.get_value::<i32>(&MAX_HEALTH),
                    "team": entity.get_value::<u64>(&TEAM),
                    "hero_id": entity.get_value::<u32>(&HERO_ID),
                    "life_state": entity.get_value::<u64>(&LIFE_STATE)
                }));
            }
        }
        players.sort_by_key(|p| p["entity_index"].as_i64());
        self.samples
            .push(json!({"tick": ctx.tick(), "players": players}));
        Ok(())
    }
}

pub fn run<R: Read + Seek>(reader: R) -> Result<Value, super::RunError> {
    let demo = DemoFile::start_reading(reader)?;
    let mut parser = Parser::from_stream_with_visitor(demo, CheckVisitor::default())?;
    parser.run_to_end()?;
    let visitor = parser.visitor_mut();
    Ok(json!({
        "entity_updates": visitor.entity_updates,
        "server_encoding": visitor.encoding,
        "player_samples": visitor.samples,
        "packets": visitor.packets.iter().map(|(packet_type, (count, bytes))|
            json!({"packet_type": packet_type, "count": count, "bytes": bytes})).collect::<Vec<_>>()
    }))
}
