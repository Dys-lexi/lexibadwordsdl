`hero-classes.json` maps hero IDs to internal class names. It was extracted from
`deadlock-api/crates/keyvalues/src/kv3_fixtures/heroes.vdata` in the adjacent
checkout. It is embedded in the binary, so the HTTP service and Docker image do
not depend on that checkout. New hero IDs without a mapping return `class: null`
while preserving `class_id`.

IDs 84–88 were added on 2026-11-17 from the published
[build 6861 hero definitions](https://assets-bucket.deadlock-api.com/assets-api-res/versions/6861/scripts/heroes.vdata.zst),
checked against the [hero assets API](https://api.deadlock-api.com/v1/assets/heroes?language=english):
84 = `hero_ratking`, 85 = `hero_chessmaster`, 86 = `hero_artist` (Violet),
87 = `hero_nurse`, and 88 = `hero_baba`. This shared lookup supplies chat,
match players, and every death-event actor. Rebuild the binary/container after
editing it because it is embedded at compile time.

`crates/valveprotos` was copied from the same checkout with its build script,
normalized protobuf definitions, and MIT license to make this workspace build
independently. The downloaded demo itself is never stored on disk by the service.
