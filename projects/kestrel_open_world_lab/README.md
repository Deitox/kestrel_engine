# Kestrel Open World Lab

Early 3D open-world capability tests (Skyrim-like slice).

## Goals

- Perspective 3D viewport as the primary runtime camera
- Basic 1st/3rd-person camera switching
- Simple “chunk streaming” prototype (spawn/despawn mesh tiles around the player)

## Controls

- `F5` Play / Pause / Resume
- `Shift+F5` Stop
- `F6` Step
- Hold `RMB` and move mouse to rotate camera (3D viewport)
- `WASD` move the player (3rd-person)
- `V` toggle camera mode (1st/3rd)

## Run

`cargo run -p kestrel_studio -- --project projects/kestrel_open_world_lab/project.kestrelproj`
