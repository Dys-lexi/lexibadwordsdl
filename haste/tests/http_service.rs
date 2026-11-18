use std::io::Write;

use axum::Router;
use axum::body::{Body, to_bytes};
use axum::http::{Request, StatusCode};
use axum::routing::get;
use haste::service::{Config, router};
use haste::valveprotos::common::{
    CDemoClassInfo, CDemoFullPacket, CDemoPacket, CDemoStringTables, CMsgPlayerInfo,
    CsvcMsgCreateStringTable,
    c_demo_string_tables::{ItemsT, TableT},
};
use haste::valveprotos::deadlock::{
    CCitadelUserMsgBossKilled, CCitadelUserMsgChatMsg, CCitadelUserMsgHeroKilled,
    CCitadelUserMsgPostMatchDetails, CMsgMatchMetaDataContents,
    c_msg_match_meta_data_contents::{MatchInfo, Players},
};
use prost::Message;
use serde_json::{Value, json};
use tower::ServiceExt;

struct Upstream {
    url: String,
    task: tokio::task::JoinHandle<()>,
}

impl Drop for Upstream {
    fn drop(&mut self) {
        self.task.abort();
    }
}

async fn upstream(app: Router) -> Upstream {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap();
    let task = tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
    Upstream {
        url: format!("http://{address}/demo.dem.bz2"),
        task,
    }
}

async fn serve_bytes(bytes: Vec<u8>) -> Upstream {
    upstream(Router::new().route(
        "/demo.dem.bz2",
        get(move || {
            let bytes = bytes.clone();
            async move { bytes }
        }),
    ))
    .await
}

fn config() -> Config {
    Config {
        allowed_replay_hosts: vec!["127.0.0.1".into()],
        ..Config::default()
    }
}

async fn request(app: Router, payload: Value) -> (StatusCode, Value) {
    let response = app
        .oneshot(
            Request::post("/parse")
                .header("content-type", "application/json")
                .body(Body::from(payload.to_string()))
                .unwrap(),
        )
        .await
        .unwrap();
    let status = response.status();
    assert_eq!(response.headers()["content-type"], "application/json");
    let bytes = to_bytes(response.into_body(), 1024 * 1024).await.unwrap();
    (status, serde_json::from_slice(&bytes).unwrap())
}

fn varint(mut value: u32, output: &mut Vec<u8>) {
    while value >= 128 {
        output.push((value as u8 & 127) | 128);
        value >>= 7;
    }
    output.push(value as u8);
}

fn chat_demo() -> Vec<u8> {
    let mut demo = b"PBDEMS2\0".to_vec();
    demo.extend_from_slice(&[0; 8]);
    // Two packets on the same tick, with no later tick to flush a pending queue.
    for text in ["hello \"world\"", "last message"] {
        let chat = CCitadelUserMsgChatMsg {
            player_slot: Some(2),
            text: Some(text.into()),
            all_chat: Some(true),
            lane_color: None,
        }
        .encode_to_vec();
        append_packet(&mut demo, 314, &chat);
    }
    demo.extend_from_slice(&[0, 42, 0]); // DEM_Stop, tick 42, empty body
    demo
}

fn append_packet(demo: &mut Vec<u8>, packet_type: u32, message: &[u8]) {
    let mut bits = Vec::<bool>::new();
    let mut write = |value: u32, count: u32| {
        for i in 0..count {
            bits.push(value & (1 << i) != 0);
        }
    };
    // These fixtures use Source 2 packet IDs between 256 and 1023 (6 + 8 bits).
    write(0x20 | (packet_type & 15), 6);
    write(packet_type >> 4, 8);
    let mut length = Vec::new();
    varint(message.len() as u32, &mut length);
    for byte in length.iter().chain(message) {
        write(u32::from(*byte), 8);
    }
    let data = bits
        .chunks(8)
        .map(|chunk| {
            chunk
                .iter()
                .enumerate()
                .fold(0, |byte, (i, bit)| byte | ((*bit as u8) << i))
        })
        .collect();
    let packet = CDemoPacket { data: Some(data) }.encode_to_vec();
    demo.extend_from_slice(&[7, 42]);
    varint(packet.len() as u32, demo);
    demo.extend(packet);
}

#[test]
fn match_start_metadata_fills_earlier_chats_and_allows_override() {
    let mut demo = chat_demo();
    demo.truncate(demo.len() - 3); // Insert metadata after both chats, before DEM_Stop.
    let metadata = CMsgMatchMetaDataContents {
        match_info: Some(MatchInfo {
            start_time: Some(1_794_547_147),
            ..Default::default()
        }),
    }
    .encode_to_vec();
    let details = CCitadelUserMsgPostMatchDetails {
        match_details: Some(metadata),
    }
    .encode_to_vec();
    append_packet(&mut demo, 316, &details);
    demo.extend_from_slice(&[0, 42, 0]);
    for (override_time, expected) in [
        (None, 1_794_547_147.0),
        (Some(1_700_000_000.0), 1_700_000_000.0),
    ] {
        let result =
            haste::runners::chat_messages::run(std::io::Cursor::new(&demo), override_time).unwrap();
        let messages = result["chat"].as_array().unwrap();
        assert_eq!(messages.len(), 2);
        assert_eq!(result["match"]["unix"].as_f64(), Some(expected));
        for message in messages {
            assert_eq!(message["unix"].as_f64(), Some(expected));
            assert!(message["game_time"].is_null()); // Unix must not depend on game time.
        }
    }
    let result = haste::runners::chat_messages::run(
        std::io::Cursor::new(chat_demo()),
        Some(1_700_000_000.0),
    )
    .unwrap();
    assert_eq!(result["chat"][0]["unix"].as_f64(), Some(1_700_000_000.0));
}

fn with_match_info(mut demo: Vec<u8>, info: MatchInfo) -> Vec<u8> {
    demo.truncate(demo.len() - 3);
    let metadata = CMsgMatchMetaDataContents {
        match_info: Some(info),
    }
    .encode_to_vec();
    let details = CCitadelUserMsgPostMatchDetails {
        match_details: Some(metadata),
    }
    .encode_to_vec();
    append_packet(&mut demo, 316, &details);
    demo.extend_from_slice(&[0, 42, 0]);
    demo
}

#[tokio::test]
async fn final_match_scoreboard_is_returned_with_chat() {
    let player = |uid, slot, team, kills, deaths, assists, net_worth| Players {
        account_id: Some(uid),
        player_slot: Some(slot),
        hero_id: Some(25),
        team: Some(team),
        kills: Some(kills),
        deaths: Some(deaths),
        assists: Some(assists),
        net_worth: Some(net_worth),
        ..Default::default()
    };
    let info = MatchInfo {
        match_id: Some(136_498_915),
        start_time: Some(1_794_547_147),
        duration_s: Some(2800),
        match_outcome: Some(0),
        winning_team: Some(1),
        game_mode: Some(1),
        match_mode: Some(1),
        players: vec![
            player(12345, 0, 0, 3, 4, 5, 10000),
            player(23456, 1, 0, 2, 6, 7, 12000),
            player(34567, 2, 1, 10, 5, 8, 25000),
            player(45678, 12, 16, 100, 100, 100, 1000000),
        ],
        team_score: vec![0, 1],
        ..Default::default()
    };
    let upstream = serve_bytes(bz2(&with_match_info(chat_demo(), info))).await;
    let (status, body) = request(router(config()).unwrap(), json!({
        "url": upstream.url, "run": ["deadlock-chat-messages"], "match_start_unix": 1_700_000_000
    })).await;
    assert_eq!(status, StatusCode::OK, "{body}");
    let result = &body["results"]["deadlock-chat-messages"];
    assert_eq!(result.as_object().unwrap().len(), 3);
    assert_eq!(result["events"], json!([]));
    assert_eq!(result["chat"].as_array().unwrap().len(), 2);
    let info = &result["match"];
    assert_eq!(info["unix"].as_f64(), Some(1_700_000_000.0));
    assert_eq!(info["match_id"], 136_498_915);
    assert_eq!(info["duration_s"], 2800);
    assert_eq!(info["winning_team"], 3);
    assert_eq!(info["winning_lobby_team"], 1);
    assert_eq!(info["game_mode"], "Normal");
    assert_eq!(info["match_mode"], "Unranked");
    assert_eq!(info["outcome"], "TeamWin");
    assert_eq!(info["has_post_match_metadata"], true);
    // This fixture has scoreboard metadata but no entity timeline. Do not use
    // a final scoreboard hero as a substitute for the requested minute snapshot.
    assert!(info["character_game_time"].is_null());
    assert_eq!(
        info["players"],
        json!([
            {"uid":12345,"account_id":12345,"player_slot":0,"team":2,"lobby_team":0,"kills":3,"deaths":4,"assists":5,"class":null,"class_id":null,"personaname":null},
            {"uid":23456,"account_id":23456,"player_slot":1,"team":2,"lobby_team":0,"kills":2,"deaths":6,"assists":7,"class":null,"class_id":null,"personaname":null},
            {"uid":34567,"account_id":34567,"player_slot":2,"team":3,"lobby_team":1,"kills":10,"deaths":5,"assists":8,"class":null,"class_id":null,"personaname":null},
            {"uid":45678,"account_id":45678,"player_slot":12,"team":null,"lobby_team":16,"kills":100,"deaths":100,"assists":100,"class":null,"class_id":null,"personaname":null}
        ])
    );
    assert_eq!(
        info["teams"],
        json!([
            {"team":2,"lobby_team":0,"player_count":2,"kills":5,"deaths":10,"assists":12,"souls":22000,"score":0},
            {"team":3,"lobby_team":1,"player_count":1,"kills":10,"deaths":5,"assists":8,"souls":25000,"score":1}
        ])
    );
}

#[test]
fn missing_metadata_draws_and_unknown_modes_do_not_invent_results() {
    let result =
        haste::runners::chat_messages::run(std::io::Cursor::new(chat_demo()), None).unwrap();
    assert_eq!(result["match"]["has_post_match_metadata"], false);
    assert_eq!(result["match"]["players"], json!([]));
    for field in [
        "unix",
        "winning_team",
        "duration_s",
        "match_id",
        "game_mode",
    ] {
        assert!(result["match"][field].is_null(), "{field}");
    }
    for team in result["match"]["teams"].as_array().unwrap() {
        assert!(team["kills"].is_null());
        assert!(team["souls"].is_null());
    }
    let mut empty_demo = b"PBDEMS2\0".to_vec();
    empty_demo.extend_from_slice(&[0; 8]);
    empty_demo.extend_from_slice(&[0, 42, 0]);
    let demo = with_match_info(
        empty_demo,
        MatchInfo {
            match_outcome: Some(2),
            winning_team: Some(0),
            game_mode: Some(999),
            players: vec![
                Players {
                    team: Some(0),
                    kills: Some(3),
                    ..Default::default()
                },
                Players {
                    team: Some(0),
                    ..Default::default()
                },
            ],
            ..Default::default()
        },
    );
    let result = haste::runners::chat_messages::run(std::io::Cursor::new(demo), None).unwrap();
    assert_eq!(result["chat"], json!([]));
    assert_eq!(result["match"]["players"].as_array().unwrap().len(), 2);
    assert!(result["match"]["players"][0]["uid"].is_null());
    assert!(result["match"]["players"][0]["player_slot"].is_null());
    assert_eq!(result["match"]["players"][0]["team"], 2);
    assert_eq!(result["match"]["outcome"], "MatchDraw");
    assert!(result["match"]["winning_team"].is_null());
    assert!(result["match"]["game_mode"].is_null());
    assert_eq!(result["match"]["game_mode_id"], 999);
    assert!(result["match"]["teams"][0]["kills"].is_null());
}

#[test]
fn death_packets_return_only_death_events_in_order() {
    let mut demo = chat_demo();
    demo.truncate(demo.len() - 3);
    let hero = CCitadelUserMsgHeroKilled {
        entindex_victim: Some(10),
        entindex_attacker: Some(11),
        entindex_scorer: Some(12),
        entindex_assisters: vec![13, 14],
        first_blood: Some(true),
        ..Default::default()
    }
    .encode_to_vec();
    append_packet(&mut demo, 319, &hero);
    let boss = CCitadelUserMsgBossKilled {
        entity_killed: Some((5 << 14) | 20),
        entity_killed_class: Some(31),
        entity_killer: Some((2 << 14) | 21),
        gametime: Some(123.0),
        objective_team: Some(3),
        objective_mask_change: Some(4),
        ..Default::default()
    }
    .encode_to_vec();
    append_packet(&mut demo, 347, &boss);
    let environmental = CCitadelUserMsgHeroKilled {
        entindex_victim: Some(15),
        entindex_attacker: Some(-1),
        entindex_scorer: Some(-1),
        victim_team_number: Some(2),
        ..Default::default()
    }
    .encode_to_vec();
    append_packet(&mut demo, 319, &environmental);
    let missing_boss = CCitadelUserMsgBossKilled {
        entity_killed: Some(16777215),
        entity_killed_class: Some(31),
        entity_killer: Some(u32::MAX),
        ..Default::default()
    }
    .encode_to_vec();
    append_packet(&mut demo, 347, &missing_boss);
    demo.extend_from_slice(&[0, 42, 0]);
    let result = haste::runners::chat_messages::run(std::io::Cursor::new(demo), None).unwrap();
    assert_eq!(result["chat"].as_array().unwrap().len(), 2);
    let events = result["events"].as_array().unwrap();
    assert_eq!(events.len(), 4);
    assert_eq!(events[0]["type"], "player_death");
    assert_eq!(events[0]["victim_entity_index"], 10);
    assert_eq!(events[0]["killer_entity_index"], 12);
    assert_eq!(events[0]["attacker"]["entity_index"], 11);
    assert_eq!(events[0]["assists"][0]["entity_index"], 13);
    assert_eq!(events[0]["assists"][1]["entity_index"], 14);
    assert!(events[0]["killer_uid"].is_null());
    assert!(events[0]["game_time"].is_null()); // No game clock in this synthetic demo.
    assert_eq!(events[1]["type"], "objective_death");
    assert_eq!(events[1]["victim_entity_index"], 20);
    assert_eq!(events[1]["victim_class_id"], 31);
    assert!(events[1]["victim_class"].is_null());
    assert!(events[1]["killer_class_id"].is_null());
    assert!(
        events
            .iter()
            .all(|event| event.get("entity_killed_class").is_none())
    );
    assert_eq!(events[1]["killer_entity_index"], 21);
    assert_eq!(events[1]["objective_team"], 3);
    assert!(events[1]["assists"].is_null());
    assert_eq!(events[2]["victim_team"], 2);
    assert_eq!(events[2]["assists"], json!([]));
    for event in events {
        assert!(event.get("victim").is_none());
        assert!(event.get("killer").is_none());
        for prefix in ["victim", "killer"] {
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
                assert!(event.get(&key).is_some(), "missing {key}");
                if (event == &events[2] && prefix == "killer") || event == &events[3] {
                    assert!(event[&key].is_null(), "expected null {key}");
                }
            }
        }
    }
}

#[test]
fn roster_personanames_come_from_userinfo_by_account_even_without_chat() {
    let mut demo = b"PBDEMS2\0".to_vec();
    demo.extend_from_slice(&[0; 8]);
    append_packet(
        &mut demo,
        44,
        &CsvcMsgCreateStringTable {
            name: Some("userinfo".into()),
            num_entries: Some(0),
            ..Default::default()
        }
        .encode_to_vec(),
    );
    let classes = CDemoClassInfo::default().encode_to_vec();
    demo.extend_from_slice(&[5, 42]);
    varint(classes.len() as u32, &mut demo);
    demo.extend(classes);
    let full = CDemoFullPacket {
        string_table: Some(CDemoStringTables {
            tables: vec![TableT {
                table_name: Some("userinfo".into()),
                items: [(222, "Player 二"), (111, "Lexi \"test\"")]
                    .into_iter()
                    .map(|(id, name)| ItemsT {
                        str: None,
                        data: Some(
                            CMsgPlayerInfo {
                                steamid: Some(76_561_197_960_265_728 + id),
                                name: Some(name.into()),
                                ..Default::default()
                            }
                            .encode_to_vec(),
                        ),
                    })
                    .collect(),
                ..Default::default()
            }],
        }),
        packet: None,
    }
    .encode_to_vec();
    demo.extend_from_slice(&[13, 42]);
    varint(full.len() as u32, &mut demo);
    demo.extend(full);
    demo.extend_from_slice(&[0, 42, 0]);
    let demo = with_match_info(
        demo,
        MatchInfo {
            players: [111, 222, 333]
                .into_iter()
                .enumerate()
                .map(|(slot, id)| Players {
                    account_id: Some(id),
                    player_slot: Some(slot as u32),
                    ..Default::default()
                })
                .collect(),
            ..Default::default()
        },
    );
    let result = haste::runners::chat_messages::run(std::io::Cursor::new(demo), None).unwrap();
    assert_eq!(result["chat"], json!([]));
    assert_eq!(
        result["match"]["players"][0]["personaname"],
        "Lexi \"test\""
    );
    assert_eq!(result["match"]["players"][1]["personaname"], "Player 二");
    assert!(result["match"]["players"][2]["personaname"].is_null());
}

fn bz2(bytes: &[u8]) -> Vec<u8> {
    let mut encoder = bzip2::write::BzEncoder::new(Vec::new(), bzip2::Compression::fast());
    encoder.write_all(bytes).unwrap();
    encoder.finish().unwrap()
}

#[tokio::test]
async fn raw_bzip2_and_zstd_return_chat_and_check_results() {
    let demo = chat_demo();
    for bytes in [
        demo.clone(),
        bz2(&demo),
        zstd::encode_all(demo.as_slice(), 1).unwrap(),
    ] {
        let upstream = serve_bytes(bytes).await;
        let (status, body) = request(
            router(config()).unwrap(),
            json!({
                "url": upstream.url, "run": ["deadlock-chat-messages", "deadlock-demo-check"]
            }),
        )
        .await;
        assert_eq!(status, StatusCode::OK, "{body}");
        let messages = body["results"]["deadlock-chat-messages"]["chat"]
            .as_array()
            .unwrap();
        assert_eq!(messages.len(), 2);
        assert_eq!(messages[0]["text"], "hello \"world\"");
        assert_eq!(messages[1]["text"], "last message");
        assert_eq!(messages[0]["tick"], 42);
        assert_eq!(messages[1]["tick"], 42);
        assert!(messages[0]["account_id"].is_null());
        assert!(messages[0]["unix"].is_null());
        assert_eq!(
            body["results"]["deadlock-demo-check"]["packets"][0]["packet_type"],
            314
        );
        assert_eq!(
            body["results"]["deadlock-demo-check"]["packets"][0]["count"],
            2
        );
    }
}

#[tokio::test]
async fn invalid_requests_are_rejected_before_download() {
    let app = router(Config::default()).unwrap();
    for payload in [
        json!({"url":"http://replay392.valve.net/demo", "run":["sh"]}),
        json!({"url":"http://replay392.valve.net/demo", "run":[]}),
        json!({"url":"http://replay392.valve.net/demo", "run":["deadlock-chat-messages","deadlock-chat-messages"]}),
        json!({"url":"http://127.0.0.1/demo", "run":["deadlock-chat-messages"]}),
        json!({"url":"http://replay392.valve.net.evil.test/demo", "run":["deadlock-chat-messages"]}),
        json!({"url":"file:///etc/passwd", "run":["deadlock-chat-messages"]}),
        json!({"url":"http://user:password@replay392.valve.net/demo", "run":["deadlock-chat-messages"]}),
        json!({"url":"http://replay392.valve.net/demo", "run":["deadlock-chat-messages"], "match_start_unix":-1}),
    ] {
        let (status, body) = request(app.clone(), payload).await;
        assert_eq!(status, StatusCode::BAD_REQUEST, "{body}");
        assert!(body["error"]["message"].is_string());
    }
}

#[tokio::test]
async fn download_and_extraction_limits_are_enforced() {
    let demo = chat_demo();
    let compressed = bz2(&demo);
    let upstream = serve_bytes(compressed.clone()).await;
    for (limit_config, code) in [
        (
            Config {
                max_download_bytes: compressed.len() - 1,
                ..config()
            },
            "download_too_large",
        ),
        (
            Config {
                max_demo_bytes: demo.len() - 1,
                ..config()
            },
            "demo_too_large",
        ),
    ] {
        let (status, body) = request(
            router(limit_config).unwrap(),
            json!({
                "url": upstream.url, "run": ["deadlock-chat-messages"]
            }),
        )
        .await;
        assert_eq!(status, StatusCode::PAYLOAD_TOO_LARGE, "{body}");
        assert_eq!(body["error"]["code"], code);
    }
    let (status, _) = request(
        router(Config {
            max_demo_bytes: demo.len(),
            ..config()
        })
        .unwrap(),
        json!({
            "url": upstream.url, "run": ["deadlock-chat-messages"]
        }),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
}

#[tokio::test]
async fn malformed_and_unsupported_demos_return_json_errors() {
    let mut truncated_bzip = bz2(&chat_demo());
    truncated_bzip.truncate(truncated_bzip.len() - 8);
    for (bytes, status, code) in [
        (
            b"<html>not a replay</html>".to_vec(),
            StatusCode::UNSUPPORTED_MEDIA_TYPE,
            "unsupported_compression",
        ),
        (
            bz2(b"not a Source 2 demo"),
            StatusCode::UNPROCESSABLE_ENTITY,
            "invalid_demo",
        ),
        (
            b"PBDEMS2\0".to_vec(),
            StatusCode::UNPROCESSABLE_ENTITY,
            "parse_failed",
        ),
        (
            truncated_bzip,
            StatusCode::UNPROCESSABLE_ENTITY,
            "decompression_failed",
        ),
    ] {
        let upstream = serve_bytes(bytes).await;
        let (actual, body) = request(
            router(config()).unwrap(),
            json!({
                "url": upstream.url, "run": ["deadlock-chat-messages"]
            }),
        )
        .await;
        assert_eq!(actual, status, "{body}");
        assert_eq!(body["error"]["code"], code);
    }
}

#[tokio::test]
async fn upstream_errors_and_disallowed_redirects_are_reported() {
    for app in [
        Router::new().route("/demo.dem.bz2", get(|| async { StatusCode::NOT_FOUND })),
        Router::new().route(
            "/demo.dem.bz2",
            get(|| async { axum::response::Redirect::temporary("http://127.0.0.2/private") }),
        ),
    ] {
        let upstream = upstream(app).await;
        let (status, body) = request(
            router(config()).unwrap(),
            json!({
                "url": upstream.url, "run": ["deadlock-chat-messages"]
            }),
        )
        .await;
        assert_eq!(status, StatusCode::BAD_GATEWAY, "{body}");
        assert_eq!(body["error"]["code"], "download_failed");
    }
}

#[tokio::test]
async fn occupied_workers_reject_another_request_and_release_after_completion() {
    let entered = std::sync::Arc::new(tokio::sync::Notify::new());
    let release = std::sync::Arc::new(tokio::sync::Notify::new());
    let upstream = upstream(Router::new().route(
        "/demo.dem.bz2",
        get({
            let entered = entered.clone();
            let release = release.clone();
            move || {
                let entered = entered.clone();
                let release = release.clone();
                async move {
                    entered.notify_one();
                    release.notified().await;
                    chat_demo()
                }
            }
        }),
    ))
    .await;
    let app = router(config()).unwrap();
    let payload = json!({"url":upstream.url, "run":["deadlock-chat-messages"]});
    let first = tokio::spawn(request(app.clone(), payload.clone()));
    tokio::time::timeout(std::time::Duration::from_secs(5), entered.notified())
        .await
        .unwrap();
    let (status, body) = request(app.clone(), payload.clone()).await;
    assert_eq!(status, StatusCode::SERVICE_UNAVAILABLE);
    assert_eq!(body["error"]["code"], "busy");
    release.notify_one();
    assert_eq!(first.await.unwrap().0, StatusCode::OK);
    release.notify_one();
    assert_eq!(request(app, payload).await.0, StatusCode::OK);
}
