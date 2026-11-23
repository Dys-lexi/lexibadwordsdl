use haste::valveprotos::common::{
    CDemoClassInfo, CDemoFullPacket, CDemoPacket, CDemoSendTables, CDemoStringTables,
    CMsgPlayerInfo, CsvcMsgCreateStringTable, CsvcMsgFlattenedSerializer, CsvcMsgPacketEntities,
    ProtoFlattenedSerializerFieldT, ProtoFlattenedSerializerT,
    c_demo_class_info::ClassT,
    c_demo_string_tables::{ItemsT, TableT},
};
use haste::valveprotos::deadlock::{
    CCitadelUserMessageItemPurchaseNotification, CCitadelUserMsgChatMsg, CitadelUserMessageIds,
};
use prost::Message;
use serde_json::{Value, json};

const STEAM_BASE: u64 = 76_561_197_960_265_728;

fn varint(mut value: u64) -> Vec<u8> {
    let mut bytes = Vec::new();
    while value >= 128 {
        bytes.push((value as u8 & 127) | 128);
        value >>= 7;
    }
    bytes.push(value as u8);
    bytes
}

#[derive(Default)]
struct Bits(Vec<bool>);

impl Bits {
    fn write(&mut self, value: u64, count: u32) {
        self.0.extend((0..count).map(|i| value & (1 << i) != 0));
    }

    fn varint(&mut self, value: u64) {
        for byte in varint(value) {
            self.write(u64::from(byte), 8);
        }
    }

    fn bytes(self) -> Vec<u8> {
        self.0
            .chunks(8)
            .map(|bits| {
                bits.iter()
                    .enumerate()
                    .fold(0, |byte, (i, bit)| byte | ((*bit as u8) << i))
            })
            .collect()
    }
}

fn command(demo: &mut Vec<u8>, kind: u8, message: impl Message) {
    let bytes = message.encode_to_vec();
    demo.extend([kind, 42]);
    demo.extend(varint(bytes.len() as u64));
    demo.extend(bytes);
}

fn packet(demo: &mut Vec<u8>, kind: u64, message: impl Message) {
    let bytes = message.encode_to_vec();
    let mut bits = Bits::default();
    // Source 2 ubitvar encoding for message IDs in 16..1024.
    if kind < 256 {
        bits.write(0x10 | (kind & 15), 6);
        bits.write(kind >> 4, 4);
    } else {
        bits.write(0x20 | (kind & 15), 6);
        bits.write(kind >> 4, 8);
    }
    bits.varint(bytes.len() as u64);
    for byte in bytes {
        bits.write(u64::from(byte), 8);
    }
    command(
        demo,
        7,
        CDemoPacket {
            data: Some(bits.bytes()),
        },
    );
}

// Controller lobby slots deliberately disagree with userinfo/chat slots.
// Sender 111 uses chat slot 0 and lobby slot 5; sender 222 uses chat slot 5
// and lobby slot 1. Slot-based matching either loses or misattributes stats.
fn player_demo(identities: &[CMsgPlayerInfo], chat_slots: &[i32]) -> Vec<u8> {
    let mut demo = b"PBDEMS2\0".to_vec();
    demo.extend([0; 8]);
    for name in ["userinfo", "instancebaseline"] {
        packet(
            &mut demo,
            44,
            CsvcMsgCreateStringTable {
                name: Some(name.into()),
                num_entries: Some(0),
                ..Default::default()
            },
        );
    }

    let mut schema = CsvcMsgFlattenedSerializer::default();
    let mut symbol = |name: &str| {
        let index = schema.symbols.len() as i32;
        schema.symbols.push(name.into());
        index
    };
    let controller = symbol("CCitadelPlayerController");
    let global = symbol("PlayerDataGlobal");
    let mut fields = Vec::new();
    for (name, ty) in [
        ("m_unLobbyPlayerSlot", "uint8"),
        ("m_steamID", "uint64"),
        ("m_iTeamNum", "uint8"),
        ("m_PlayerDataGlobal", "PlayerDataGlobal"),
        ("m_nHeroID", "uint32"),
        ("m_iPlayerKills", "int32"),
        ("m_iDeaths", "int32"),
        ("m_iPlayerAssists", "int32"),
        ("m_iGoldNetWorth", "int32"),
    ] {
        fields.push(ProtoFlattenedSerializerFieldT {
            var_name_sym: Some(symbol(name)),
            var_type_sym: Some(symbol(ty)),
            field_serializer_name_sym: (name == "m_PlayerDataGlobal").then_some(global),
            field_serializer_version: (name == "m_PlayerDataGlobal").then_some(0),
            ..Default::default()
        });
    }
    schema.fields = fields;
    // Child serializers must precede their parent.
    schema.serializers = vec![
        ProtoFlattenedSerializerT {
            serializer_name_sym: Some(global),
            serializer_version: Some(0),
            fields_index: (4..9).collect(),
        },
        ProtoFlattenedSerializerT {
            serializer_name_sym: Some(controller),
            serializer_version: Some(0),
            fields_index: (0..4).collect(),
        },
    ];
    let bytes = schema.encode_to_vec();
    let mut data = varint(bytes.len() as u64);
    data.extend(bytes);
    command(&mut demo, 4, CDemoSendTables { data: Some(data) });
    command(
        &mut demo,
        5,
        CDemoClassInfo {
            classes: vec![ClassT {
                class_id: Some(0),
                network_name: Some("CCitadelPlayerController".into()),
                ..Default::default()
            }],
        },
    );
    command(
        &mut demo,
        13,
        CDemoFullPacket {
            string_table: Some(CDemoStringTables {
                tables: vec![
                    TableT {
                        table_name: Some("userinfo".into()),
                        items: identities
                            .iter()
                            .map(|info| ItemsT {
                                str: None,
                                data: Some(info.encode_to_vec()),
                            })
                            .collect(),
                        ..Default::default()
                    },
                    TableT {
                        table_name: Some("instancebaseline".into()),
                        // Empty baseline: FieldPathEncodeFinish, stream bits 1,0.
                        items: vec![ItemsT {
                            str: Some("0".into()),
                            data: Some(vec![1]),
                        }],
                        ..Default::default()
                    },
                ],
            }),
            packet: None,
        },
    );

    let mut bits = Bits::default();
    for (slot, account, team, hero, kills, deaths, assists, souls) in [
        (5, 111, 2, 13, 8, 9, 22, 62663),
        (1, 222, 3, 11, 14, 10, 18, 44299),
    ] {
        bits.write(1, 6); // Entity index delta: controllers at indices 1 and 3.
        bits.write(2, 2); // CREATE; a single class needs zero class-ID bits.
        bits.write(0, 17); // Serial number.
        bits.varint(0);
        bits.write(0, 3); // PlusOne x3: slot, Steam ID, team.
        bits.write(0b01011011, 8); // PushOneLeftDeltaOneRightZero: [3,0].
        bits.write(0, 4); // PlusOne x4: remaining global stats.
        bits.write(1, 2); // FieldPathEncodeFinish.
        for value in [slot, STEAM_BASE + account, team, hero] {
            bits.varint(value);
        }
        for value in [kills, deaths, assists, souls] {
            bits.varint(value * 2); // Positive signed integers use zigzag encoding.
        }
    }
    packet(
        &mut demo,
        55,
        CsvcMsgPacketEntities {
            updated_entries: Some(2),
            entity_data: Some(bits.bytes()),
            ..Default::default()
        },
    );
    for slot in chat_slots {
        packet(
            &mut demo,
            314,
            CCitadelUserMsgChatMsg {
                player_slot: Some(*slot),
                text: Some("hello".into()),
                ..Default::default()
            },
        );
    }
    demo
}

fn parse_chat(identities: &[CMsgPlayerInfo], chat_slots: &[i32]) -> Value {
    let mut demo = player_demo(identities, chat_slots);
    demo.extend([0, 42, 0]);
    haste::runners::chat_messages::run(std::io::Cursor::new(demo), None).unwrap()
}

#[test]
fn chat_stats_follow_sender_identity_not_lobby_slot() {
    let mut identities = vec![CMsgPlayerInfo::default(); 6];
    identities[0].steamid = Some(STEAM_BASE + 111);
    identities[5].steamid = Some(STEAM_BASE + 222);
    let result = parse_chat(&identities, &[0, 5]);
    let messages = result["chat"].as_array().unwrap();
    for (message, expected) in messages.iter().zip([
        json!([111, 0, 2, 13, 8, 9, 22, 62663]),
        json!([222, 5, 3, 11, 14, 10, 18, 44299]),
    ]) {
        let actual: Vec<_> = [
            "account_id",
            "player_slot",
            "team",
            "class_id",
            "kills",
            "deaths",
            "assists",
            "souls",
        ]
        .map(|key| message[key].clone())
        .into();
        assert_eq!(json!(actual), expected);
    }
}

#[test]
fn unknown_sender_never_borrows_a_matching_lobby_slots_stats() {
    let mut identities = vec![CMsgPlayerInfo::default(); 6];
    identities[5].steamid = Some(STEAM_BASE + 999);
    for result in [parse_chat(&identities, &[5]), parse_chat(&[], &[5])] {
        let message = &result["chat"][0];
        for key in [
            "team", "class_id", "kills", "deaths", "assists", "souls", "build",
        ] {
            assert!(message[key].is_null(), "{key}: {message}");
        }
    }
}

#[test]
fn chat_can_resolve_userinfo_xuid() {
    let result = parse_chat(
        &[CMsgPlayerInfo {
            xuid: Some(STEAM_BASE + 111),
            ..Default::default()
        }],
        &[0],
    );
    assert_eq!(result["chat"][0]["account_id"], 111);
    assert_eq!(result["chat"][0]["kills"], 8);
}

#[test]
fn each_shop_notification_gets_a_row_with_the_players_current_stats() {
    let identities = [CMsgPlayerInfo {
        steamid: Some(STEAM_BASE + 111),
        ..Default::default()
    }];
    let mut demo = player_demo(&identities, &[]);
    for (slot, sell) in [(0, None), (0, Some(true)), (0, Some(false)), (-1, None)] {
        packet(
            &mut demo,
            CitadelUserMessageIds::KEUserMsgItemPurchaseNotification as u64,
            CCitadelUserMessageItemPurchaseNotification {
                userid: Some(slot),
                ability_id: Some(4000000000),
                sell,
                quickbuy: None,
            },
        );
    }
    demo.extend([0, 42, 0]);
    let result = haste::runners::chat_messages::run(std::io::Cursor::new(demo), None).unwrap();
    let items = result["items"].as_array().unwrap();
    assert_eq!(items.len(), 4);
    for (row, bought) in items.iter().zip([true, false, true]) {
        assert_eq!(
            *row,
            json!({
                "account_id": 111, "tick": 42, "game_time": null, "imbuedability": [], "item": 4000000000u32,
                "kills": 8, "deaths": 9, "assists": 22, "souls": 62663, "bought": bought
            })
        );
    }
    assert_eq!(items[3]["item"], 4000000000u32);
    assert!(items[3]["account_id"].is_null());
    assert!(items[3]["souls"].is_null());
    assert_eq!(parse_chat(&identities, &[])["items"], json!([]));
}
