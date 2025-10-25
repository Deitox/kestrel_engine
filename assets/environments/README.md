# Environment Maps

Drop `.hdr`, `.exr`, or `.png` equirectangular maps in this folder and the engine will auto-register them at startup.

- File names are converted into environment keys using the pattern `environment::<file_stem>` (non-alphanumeric chars become `_`).
- Registered environments appear in the in-app dropdown and can be referenced by scenes; paths are preserved in scene dependencies so they can reload on other machines.
- Use smaller test maps (e.g., 256x128) when iterating to keep load times fast. High-res HDRIs work as well, but expect a longer preprocessing pass as diffuse/specular cubemaps are generated.

Example:

```
assets/environments/
  studio.hdr        -> environment::studio
  neon-alley.exr    -> environment::neon_alley
```