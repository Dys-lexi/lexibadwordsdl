//! Stable projections of CMsgMatchMetaDataContents and its final scoreboard.

use crate::valveprotos::deadlock::{ECitadelGameMode, ECitadelMatchMode};
use serde_json::{Value, json};

// Read only the fields needed here. Unrelated nested stats have changed wire
// types between game versions and should not prevent extracting the scoreboard.
#[derive(prost::Message)]
pub(super) struct MatchMetadata {
    #[prost(message, optional, tag = "2")]
    pub match_info: Option<MatchInfo>,
}

#[derive(prost::Message)]
pub(super) struct MatchInfo {
    #[prost(uint32, optional, tag = "1")]
    pub duration_s: Option<u32>,
    #[prost(int32, optional, tag = "2")]
    pub match_outcome: Option<i32>,
    #[prost(int32, optional, tag = "3")]
    pub winning_team: Option<i32>,
    #[prost(message, repeated, tag = "4")]
    pub players: Vec<Player>,
    #[prost(uint32, optional, tag = "5")]
    pub start_time: Option<u32>,
    #[prost(uint64, optional, tag = "6")]
    pub match_id: Option<u64>,
    #[prost(int32, optional, tag = "9")]
    pub game_mode: Option<i32>,
    #[prost(int32, optional, tag = "10")]
    pub match_mode: Option<i32>,
    #[prost(uint32, repeated, tag = "28")]
    pub team_score: Vec<u32>,
}

#[derive(prost::Message)]
pub(super) struct Player {
    #[prost(uint32, optional, tag = "1")]
    pub account_id: Option<u32>,
    #[prost(uint32, optional, tag = "2")]
    pub player_slot: Option<u32>,
    #[prost(int32, optional, tag = "6")]
    pub team: Option<i32>,
    #[prost(uint32, optional, tag = "8")]
    pub kills: Option<u32>,
    #[prost(uint32, optional, tag = "9")]
    pub deaths: Option<u32>,
    #[prost(uint32, optional, tag = "10")]
    pub assists: Option<u32>,
    #[prost(uint32, optional, tag = "11")]
    pub net_worth: Option<u32>,
}

fn entity_team(lobby_team: i32) -> Option<i32> {
    match lobby_team {
        0 => Some(2),
        1 => Some(3),
        _ => None,
    }
}

impl MatchInfo {
    pub fn summary(
        &self,
        unix: Option<f64>,
        game_build: Option<i32>,
        map: Option<&str>,
        present: bool,
    ) -> Value {
        // A draw/error has no winner, even if the protobuf contains a default team.
        let winning_lobby_team = self.winning_team.filter(|team| {
            entity_team(*team).is_some() && self.match_outcome.is_none_or(|outcome| outcome == 0)
        });
        let teams: Vec<Value> = (0..=1)
            .map(|lobby_team| {
                let players: Vec<&Player> = self
                    .players
                    .iter()
                    .filter(|player| player.team == Some(lobby_team))
                    .collect();
                let total = |field: fn(&Player) -> Option<u32>| -> Option<u64> {
                    if players.is_empty() {
                        return None;
                    }
                    players
                        .iter()
                        .map(|player| field(player).map(u64::from))
                        .sum()
                };
                json!({
                    "team": entity_team(lobby_team),
                    "lobby_team": lobby_team,
                    "player_count": players.len(),
                    "kills": total(|p| p.kills),
                    "deaths": total(|p| p.deaths),
                    "assists": total(|p| p.assists),
                    "souls": total(|p| p.net_worth),
                    "score": self.team_score.get(lobby_team as usize),
                })
            })
            .collect();
        let players: Vec<Value> = self
            .players
            .iter()
            .map(|player| {
                json!({
                    "uid": player.account_id,
                    "account_id": player.account_id,
                    "player_slot": player.player_slot,
                    "team": player.team.and_then(entity_team),
                    "lobby_team": player.team,
                    "kills": player.kills,
                    "deaths": player.deaths,
                    "assists": player.assists,
                })
            })
            .collect();
        let game_mode = self
            .game_mode
            .and_then(|id| ECitadelGameMode::try_from(id).ok())
            .map(|mode| mode.as_str_name().trim_start_matches("k_ECitadelGameMode_"));
        let match_mode = self
            .match_mode
            .and_then(|id| ECitadelMatchMode::try_from(id).ok())
            .map(|mode| {
                mode.as_str_name()
                    .trim_start_matches("k_ECitadelMatchMode_")
            });
        let outcome = match self.match_outcome {
            Some(0) => Some("TeamWin"),
            Some(1) => Some("Error"),
            Some(2) => Some("MatchDraw"),
            _ => None,
        };
        json!({
            "match_id": self.match_id,
            "unix": unix,
            "duration_s": self.duration_s,
            "winning_team": winning_lobby_team.and_then(entity_team),
            "winning_lobby_team": winning_lobby_team,
            "game_mode": game_mode,
            "game_mode_id": self.game_mode,
            "match_mode": match_mode,
            "match_mode_id": self.match_mode,
            "outcome": outcome,
            "outcome_id": self.match_outcome,
            "teams": teams,
            "players": players,
            "game_build": game_build,
            "map": map,
            "has_post_match_metadata": present,
        })
    }
}
