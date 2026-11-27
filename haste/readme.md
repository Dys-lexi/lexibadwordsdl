# Haste coordinator client

Set `COORDINATOR_ENDPOINT` and `TOKEN` in the repository's `.env`, then start Haste
from the repository root with `docker compose up --build -d haste`.

Haste keeps one shared queue of replay URLs. Each free worker takes the next URL.
Haste keeps up to `DEMO_SURPLUS` URLs waiting in addition to active work.
It posts `{"count": N}` to `COORDINATOR_ENDPOINT/geturls` for the number of free
worker slots plus the shortfall in the waiting queue. Only one refill runs at a
time. Workers continue taking queued URLs while it runs. The coordinator
returns a JSON array of URL strings.
Haste queues every URL in a partial batch and requests the remaining shortfall.
An empty array or empty successful response starts a 30-second wait before the
next refill. Workers continue processing and submitting demos during the wait.

Haste posts completed replays to `COORDINATOR_ENDPOINT/submitdemo` as a JSON
array, including when only one replay is ready. Both coordinator requests carry
the `Token` header. A base URL ending in `/abc` gives the routes `/abc/geturls`
and `/abc/submitdemo`.

One sender handles submissions while workers continue parsing. If a POST fails,
Haste keeps the failed matches in memory and adds newly completed matches to
the next attempt. It retries after 2 seconds and doubles the delay on each
failure, up to 60 seconds, until the coordinator returns a successful HTTP
status. The coordinator should accept repeat submissions for a match if a
previous acknowledgement was lost.

Example submission with one completed replay

```json
[
  {
    "results": {
      "deadlock-chat-messages": {
        "match": {},
        "chat": [],
        "events": [],
        "items": []
      }
    }
  }
]
```

The `chat` array contains the existing messages and their player state at the
message's tick. The `match` object contains:

- `unix`: match start timestamp in Unix seconds.
- `match_id`, `duration_s`, `map`, and `game_build`.
- `winning_team`: entity team ID (`2` or `3`), consistent with `chat[].team`.
  `winning_lobby_team` preserves the protobuf's team ID (`0` or `1`).
- `game_mode` and `match_mode`: readable names such as `Normal` and `Unranked`,
  with numeric `game_mode_id` and `match_mode_id` alongside them.
- `outcome` and `outcome_id`: e.g. `TeamWin` / `0` or `MatchDraw` / `2`.
- `players`: the final metadata's player roster, including players who never
  chatted. Each entry has `uid` (the 32-bit Steam account ID, also exposed as
  `account_id` to match chat messages), `player_slot`, `team` (`2` or `3`), and
  `lobby_team` (`0` or `1`). `personaname` is their recorded in-match name,
  matched by account ID (including players who never chatted), not a live Steam
  profile lookup. Unknown values are null; without final metadata the
  roster is empty. Spectators/unknown teams retain their raw `lobby_team`, with
  `team` set to null. Each player also includes final `kills`, `deaths`, and
  `assists` from the scoreboard, plus `class` and `class_id` for their character.
- `character_game_time`: the game-clock time of the character snapshot used for
  `players`. Characters are sampled at the first completed recorded tick at or
  after 60 seconds; matches shorter than 60 seconds use the starting snapshot
  (the first completed recorded tick at or after game time zero). The clock
  accounts for paused ticks. Later character changes do not alter this snapshot.
  Missing character state is null; final scoreboard heroes are not substituted.
- `teams`: one entry per playing team, containing `team`, `lobby_team`,
  `player_count`, final `kills`, `deaths`, `assists`, `souls`, and `score`.
  Kills/KDA/souls are summed from the final player scoreboard; `score` is the
  separate mode-specific score provided by the metadata.
- `has_post_match_metadata`: whether the demo supplied its final match details.

Final totals come from `PostMatchDetails`, including for players who disconnected
before the last chat. Missing totals remain null, and draws/errors have no winner.
Unknown mode IDs are preserved with null names. Match details are returned even
when there are no chat messages. Existing fields inside each chat message remain.

The `items` array contains one row per shop notification in demo order, including
repeat purchases, sales, and refunds. Each row has the player's `account_id`, the
replay `tick` of the shop notification,
the same `game_time` in seconds used by chat, the numeric `item` ID, and
`bought` (true for a purchase, false for a sale). `imbuedability` is an array
of numeric ability IDs in replay order, or `[]` when no target is available. `kills`,
`deaths`, `assists`, and `souls` (net worth) come from the player's controller
at the transaction tick. If the live notification and item entity leave the player
unknown, Haste checks the demo's embedded post-match item history for a unique
purchase or sale of that item at that time. The stats still come from the
transaction tick. Unresolved identities and unavailable stats are null.

The sibling `events` array contains only `player_death` and `objective_death`
events, in replay order. Each event includes `tick`, `game_time`, and `assists`.
Victim and killer fields are flat: `victim_account_id`, `killer_account_id`,
`victim_class`, `killer_class`, etc. Both prefixes include `uid`, `account_id`,
`player_slot`, `team`, `class`, `class_id`, `entity_index`, and `entity_class`.
There are no nested `victim` or `killer` objects. Unavailable fields are null;
the packet's explicit `victim_team` is preserved even if its entity is missing.
`attacker` and entries inside `assists` remain actor objects.

Player deaths use `HeroKilled`, including the credited scorer as `killer`, the
actual `attacker`, and the packet's assister list (`[]` means no assisters).
Objective deaths use `BossKilled`, with objective team/mask and bosses remaining.
`victim_class_id` contains the hero ID for player deaths and the raw
`BossKilled.entity_killed_class` code for objective deaths; these are separate
ID systems, distinguished by the event type. There is no separate
`entity_killed_class` output field. Objective class codes are not mapped to hero
names; use `victim_entity_class` for the entity's network class name.
That packet does not contain assists, so
objective `assists` is null (unavailable). Objective `server_game_time` preserves
the raw packet timestamp; `game_time` uses the same match clock as chat. If the
demo lacks clock state, `game_time` remains null. Actor character fields reflect
the event's state, independently of the one-minute roster snapshot.

The client runs `deadlock-chat-messages` for each URL. Downloads, decompression,
and parsing happen in memory. Haste detects bzip2, Zstandard (including Valve
files named `.dem.bz2`), and raw Source 2 demos by their contents.

Chat fields preserve the existing extractor: `class` is the internal hero class,
`souls` is net worth, team souls are the sum of player net worth, and `build` is
base64 of the selected hero guide's serialized bytes. `build_id` is its ID;
`game_build` is the demo's game build number. Builds use the last observed state
when a player's pawn is unavailable. Unknown values remain null.
Chat `player_slot` indexes the replay's `userinfo` table; it is not a lobby slot.
Player stats, team, hero, and build are resolved by the sender's Steam/account ID.
If the sender cannot be matched to a controller, those fields remain null.

`game_time` is seconds on the game clock. `match.unix` and `chat[].unix` are the
match's start timestamp in Unix seconds, read from its `PostMatchDetails` metadata. Every chat
message gets the same start timestamp, including messages recorded before that
metadata arrives. If an incomplete demo has no start metadata, `unix` is null.

Configuration (environment variables):

| Variable | Default | Meaning |
| --- | --- | --- |
| `COORDINATOR_ENDPOINT` | required | Coordinator base URL including any path prefix |
| `TOKEN` | required | Shared coordinator token |
| `DEMO_SURPLUS` | `0` | Target number of URLs waiting for a worker |
| `MAX_DOWNLOAD_BYTES` | `536870912` | Maximum downloaded size (512 MiB) |
| `MAX_DOWNLOAD_SPEED` | `0` | Combined replay download rate in Mbps (`0` is unlimited) |
| `MAX_DEMO_BYTES` | `2147483648` | Maximum decompressed size (2 GiB) |
| `MAX_CONCURRENT_REQUESTS` | `1` | Downloads/parses allowed at once |
| `DOWNLOAD_READ_TIMEOUT_SECONDS` | `30` | Time allowed for replay response headers or between body reads |
| `ALLOWED_REPLAY_HOSTS` | empty | Default: `replay<number>.valve.net`; override with exact comma-separated hostnames |
| `MEMORY_LIMIT` | `4g` | Compose container memory limit |

URL validation also applies to redirects. Overrides are useful for a private
replay mirror or local testing. Budget memory for the compressed data,
decompressed demo, parser state, and JSON output per worker.

To run locally with Rust and `protoc` installed:

```sh
# Export COORDINATOR_ENDPOINT and TOKEN first.
cargo run --release --bin haste-client
cargo test -p haste --locked
```

The standalone chat example remains available:

```sh
cargo run --release --example deadlock-chat-messages -- /path/to/demo.dem
```

---

# haste

world's fastest dota 2 and deadlock (the game) replay parser. more then two
times faster than comically fast.

haste attempts to squeeze maximum single-core performance from the cpu, which
enables efficient utilization of all cores for parsing multiple replays
simultaneously.

haste does not want to be very user-friendly, it provides you with a relatively
low-level access to correct usable data. it up to you how you structure your
programs. you may choose to build your own nice api layer. anything is possible,
theoretically even something as silly (i love silly) as ecs is doable (think of
bevy game engine).

> [!WARNING]
> there are many `unsafe`s in the codebase, some are rational and explained,
> some need to be let go; public api isn't great and can be considered very
> unstable (e.g. it is not final and parts of it may change dramatically).

## how to

### examples

notable examples to check out for detailed usage:

- [deadlock-demo-check](examples/deadlock-demo-check.rs) parses a deadlock replay end
  to end and prints sanity checks (server-announced encoding params, player positions /
  health / hero ids / net worth, packet message histogram); useful after game updates.

- [deadlock-position](examples/deadlock-position.rs) demonstrates how to work with
  entities and how to get player (or any other entity, if desired) positions in
  deadlock (the game);
- [deadlock-gametime](examples/deadlock-gametime.rs) also demonstrates how to work
  with entities and how to compute game time (not a very straightforward thing to
  do, thanks valve).
- [dota2-allchat](examples/dota2-allchat.rs) shows how to work with packet
  messages.

to run these examples navigate to haste directory and run

```console
$ cargo run --example <example-name> -- <path-to-dem-file>
```

### usage

to use haste in your project, you'll need :

- `protoc` (protocol buffer compiler) in your `$PATH`, or `$PROTOC` environment
  variable needs point to it

haste is not published to [crates.io](https://crates.io/) (yet?). you can add it
to your `Cargo.toml` as a [git
dependency](https://doc.rust-lang.org/cargo/reference/specifying-dependencies.html#specifying-dependencies-from-git-repositories),

```toml
[dependencies]
haste = { git = "https://github.com/blukai/haste.git" }
```

haste's entity representatin is not debugger / print friendly, you would want to
use [haste-inspector](https://github.com/blukai/haste-inspector) (replay dev
tools) to explore all the entities that are present in replays.

## feature flags

- `broadcast`: enables http broadcasts.
- `deadlock`: enables deadlock protos and some utilities.
- `dota2`: enabled dota2 protos and some utilities.

## benchmarks

TODO: benchmarks and comparisons with other projects such as clarity and manta.

to tease a bit.. as of 25-09-2024 in standard release build with no extra
optimizations nor non-stadard memory allocators:

- 31 minutes deadlock match can be parsed in ~660 ms with ~17.5 mb peak memory
  consumtion;
- 38 minutes dota 2 captains mode match - in ~650 ms with ~18 mb peak memory
  consumtion.

run time is an average from 10 runs with no warmups. peak memory consumtion is
`time`'s `maximum resident set size` stat.

## motivation, huh?

why create another replay parser?
to prove myself that i'm right (long story; the proof was found quicly, but then
i fell down the rabbit hole of further performance optimizations).

## credits / references

valve's official repos and wiki provide quite a handful of useful information,
but special credits go to [invokr](https://github.com/invokr) who worked on
[dotabuff/manta](https://github.com/dotabuff/manta) and to
[spheenik](https://github.com/spheenik) the creator of
[skadistats/clarity](https://github.com/skadistats/clarity) (i have not personally
interacted with either of them).

other notable resources:

- [ButterflyStats/butterfly](https://github.com/ButterflyStats/butterfly)
- [ValveSoftware/csgo-demoinfo](https://github.com/ValveSoftware/csgo-demoinfo)
- [ValveSoftware/source-sdk-2013](https://github.com/ValveSoftware/source-sdk-2013)
- [SwagSoftware/Kisak-Strike](https://github.com/SwagSoftware/Kisak-Strike)
- [markus-wa/demoinfocs-golang](https://github.com/markus-wa/demoinfocs-golang)
  (some info about how pointer serializer fields work)
- [demofile-net](https://github.com/saul/demofile-net) (tv broadcasts)

## performance / profiling

- [hotspot](https://github.com/KDAB/hotspot)
- [heaptrack](https://github.com/KDE/heaptrack)
- [perf](https://perf.wiki.kernel.org/index.php/Main_Page)
- [hyperfine](https://github.com/sharkdp/hyperfine)
- [asheplyakov/branchmiss](https://github.com/asheplyakov/branchmiss)

and some more can probably be found across comments in the codebase.

### Replay downloads

Haste probes the replay server with a one-byte range request. Files of at least
8 MiB use eight parallel requests when the server supports ranges. Smaller
files and servers that reject ranges use one sequential stream. If the probe
already returns a full response, Haste reads it directly. A failed parallel
download falls back to one sequential request. [Deadlock API downloader](https://github.com/deadlock-api/deadlock-api/blob/master/api/src/routes/v1/matches/demo/download.rs)

The connection timeout is 5 seconds and the read timeout defaults to 30 seconds.
Each successful read resets that timeout, so a slow transfer can run as long as
it keeps delivering data.

### Diagnostic logs

Haste writes one JSON object per diagnostic event to stderr. Follow them with:

```sh
docker compose logs -f --no-log-prefix haste
```

Each event includes the download ID, match ID when available, replay host,
timestamp, elapsed time, bytes received, and total size when known. Events show
whether Haste used ranges or a sequential stream, why it fell back, and the
resulting transfer speed or error. Decompression and parsing events share the
same download ID.
