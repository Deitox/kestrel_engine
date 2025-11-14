# Plugin Asset Readbacks

Isolated plugins cannot touch project files directly, so the engine exposes a gated readback channel.
Entries in `config/plugins.json` must declare `asset_filters` plus the `assets` capability before any
readback succeeds. Every request is throttled (8 per 16 ms window, 4 MB per window) and recorded by
the analytics plugin (`plugin_asset_readback` events surface in the Stats panel and telemetry).

## Payloads

| Kind         | Description                                                                                          |
|--------------|------------------------------------------------------------------------------------------------------|
| `AtlasMeta`  | Serializes sprite atlas metadata (regions, animations, dimensions) as JSON.                          |
| `AtlasBinary`| Streams the raw atlas texture bytes (PNG/JPG) so plugins can hash or mirror the source asset.        |
| `BlobRange`  | Returns an arbitrary byte range from a logical blob id (e.g. audio banks) with manifest-supplied ACL.|

All metadata responses follow the JSON schema stored in `docs/plugins/atlas_meta_schema.json`. An example:

```json
{
  "atlas_id": "ui/hud",
  "width": 2048,
  "height": 2048,
  "image_path": "assets/images/ui/hud.png",
  "regions": [
    {
      "name": "button_idle",
      "rect": [0, 0, 128, 64],
      "uv": [0.0, 0.0, 0.0625, 0.03125],
      "id": 2
    }
  ],
  "animations": [
    {
      "name": "button_hover",
      "looped": true,
      "loop_mode": "PingPong",
      "frame_count": 4,
      "frames": [
        { "region": "button_idle", "duration": 0.08, "events": ["hover_start"] }
      ]
    }
  ]
}
```

## Helper Types

### TypeScript

```ts
export interface AtlasMetaDocument {
  atlas_id: string;
  width: number;
  height: number;
  image_path: string;
  regions: AtlasMetaRegion[];
  animations: AtlasMetaAnimation[];
}

export interface AtlasMetaRegion {
  name: string;
  rect: [number, number, number, number];
  uv: [number, number, number, number];
  id: number;
}

export interface AtlasMetaAnimation {
  name: string;
  looped: boolean;
  loop_mode: string;
  frame_count: number;
  frames: AtlasMetaAnimationFrame[];
}

export interface AtlasMetaAnimationFrame {
  region: string;
  duration: number;
  events: string[];
}
```

### Rust

```rust
#[derive(Debug, serde::Deserialize)]
pub struct AtlasMetaDocument {
    pub atlas_id: String,
    pub width: u32,
    pub height: u32,
    pub image_path: String,
    pub regions: Vec<AtlasMetaRegion>,
    pub animations: Vec<AtlasMetaAnimation>,
}

#[derive(Debug, serde::Deserialize)]
pub struct AtlasMetaRegion {
    pub name: String,
    pub rect: [u32; 4],
    pub uv: [f32; 4],
    pub id: u16,
}
```

## Tooling

- `isolated_plugin_cli --asset-readback kind=value` lets QA and CI validate access (add `--fail-on-throttle`
  to treat throttling as an error).
- The editor’s Plugin panel exposes a “Retry asset readback” button that replays the most recent request.
- Analytics events (`plugin_asset_readback`) capture plugin id, payload kind/target, byte count, cache hits,
  and duration; the Stats panel renders the latest entries for manual inspection.
