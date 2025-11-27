# GameKit Quickstart

Path: `assets/scenes/gamekit_sample_scene.json`

What it shows:
- The `gamekit_host` runs `assets/scripts/gamekit_sample.rhai`, spawning a player and waves of light/heavy/dash enemies from the GameKit template helpers. It auto-buys a few upgrades (income, damage, spawn haste, auto-repair, shield overload) and logs score/scrap/wave as it progresses.
- Kit collision events drive damage: enemies emit `kit_enemy_contact` with `#{ damage, source, target }`, the player emits `kit_enemy_hit` with `#{ damage, target }`. The sample listens for both and routes into `kit::on_player_hit` / `kit::enemy_hit_handle`.
- Shared stats live in `stat_*`: `score`, `scrap`, `wave`, `lives`. Watch the console for `kit:score=` and `kit:stats` lines; you can also read them from a script via `stat_get`.

How to run:
- Open the scene in Studio, press Play; you should see log output in the Scripts console as waves advance.
- Modify `assets/scripts/gamekit_sample.rhai` to tweak spawn positions, upgrade costs, or add your own event listeners. Hot reload will re-run the script with the new flow.

Notes:
- The placed entities are spawned by the kit templates so collision events target the correct handles automatically. Prefab aliases for the dash enemy live in `assets/prefabs/aliases.json` alongside the light/heavy aliases.
- The ScriptWorld reference panel in Studio includes a GameKit entry describing these events and stat keys. Use it as a reminder while scripting.
