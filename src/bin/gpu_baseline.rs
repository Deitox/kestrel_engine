use anyhow::{anyhow, Context, Result};
use glam::Vec3;
use kestrel_engine::assets::AssetManager;
use kestrel_engine::camera::Camera2D;
use kestrel_engine::camera3d::Camera3D;
use kestrel_engine::config::WindowConfig;
use kestrel_engine::ecs::{EcsWorld, InstanceData};
use kestrel_engine::environment::EnvironmentRegistry;
use kestrel_engine::gpu_baseline::{compare_baselines, GpuBaselineSnapshot, GpuTimingAccumulator};
use kestrel_engine::material_registry::MaterialRegistry;
use kestrel_engine::mesh_registry::MeshRegistry;
use kestrel_engine::renderer::{MeshDraw, RenderViewport, Renderer, SpriteBatch};
use kestrel_engine::scene::Scene;
use std::collections::HashMap;
use std::env;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::{SystemTime, UNIX_EPOCH};

fn main() -> Result<()> {
    let args = BaselineArgs::parse(env::args().skip(1))?;
    pollster::block_on(run_baseline(args))
}

#[derive(Debug)]
struct BaselineArgs {
    frames: usize,
    output: PathBuf,
    baseline: Option<PathBuf>,
    default_tolerance_ms: f32,
    pass_tolerances: HashMap<String, f32>,
}

impl BaselineArgs {
    fn parse<I, S>(args: I) -> Result<Self>
    where
        I: IntoIterator<Item = S>,
        S: Into<String>,
    {
        let mut frames = 240usize;
        let mut output = PathBuf::from("perf/gpu_baseline.json");
        let mut baseline = None;
        let mut default_tol = 0.30f32;
        let mut pass_tolerances = HashMap::from([
            ("Shadow pass".to_string(), 0.30),
            ("Mesh pass".to_string(), 0.20),
            ("Sprite pass".to_string(), 0.15),
        ]);
        let mut iter = args.into_iter();
        while let Some(raw) = iter.next() {
            let arg = raw.into();
            match arg.as_str() {
                "--frames" => {
                    let value: String =
                        iter.next().ok_or_else(|| anyhow!("--frames requires a value"))?.into();
                    frames = value.parse().context("invalid --frames value")?;
                }
                "--output" => {
                    let value: String =
                        iter.next().ok_or_else(|| anyhow!("--output requires a value"))?.into();
                    output = PathBuf::from(value);
                }
                "--baseline" => {
                    let value: String =
                        iter.next().ok_or_else(|| anyhow!("--baseline requires a value"))?.into();
                    baseline = Some(PathBuf::from(value));
                }
                "--default-tolerance" => {
                    let value: String =
                        iter.next().ok_or_else(|| anyhow!("--default-tolerance requires a value"))?.into();
                    default_tol = value.parse().context("invalid --default-tolerance value")?;
                }
                "--pass-tolerance" => {
                    let value: String =
                        iter.next().ok_or_else(|| anyhow!("--pass-tolerance expects label=value"))?.into();
                    let mut split = value.splitn(2, '=');
                    let label = split.next().unwrap_or_default();
                    let tol = split
                        .next()
                        .ok_or_else(|| anyhow!("--pass-tolerance expects label=value"))?
                        .parse()
                        .context("invalid pass tolerance value")?;
                    pass_tolerances.insert(label.to_string(), tol);
                }
                other => return Err(anyhow!("Unknown argument '{other}'")),
            }
        }
        Ok(Self { frames, output, baseline, default_tolerance_ms: default_tol, pass_tolerances })
    }
}

async fn run_baseline(args: BaselineArgs) -> Result<()> {
    if let Some(parent) = args.output.parent() {
        if !parent.as_os_str().is_empty() {
            std::fs::create_dir_all(parent).with_context(|| format!("creating {}", parent.display()))?;
        }
    }
    let mut renderer = Renderer::new(&WindowConfig {
        title: "GPU Baseline".into(),
        width: 1280,
        height: 720,
        vsync: false,
        fullscreen: false,
    })
    .await;
    renderer.init_headless_for_test().await?;
    if !renderer.gpu_timing_supported() {
        return Err(anyhow!(
            "GPU timestamp queries unavailable on the selected adapter/backend.\n\
             Baseline capture requires both TIMESTAMP_QUERY and TIMESTAMP_QUERY_INSIDE_ENCODERS.\n\
             On Windows try forcing DX12 (`$env:WGPU_BACKEND = \"dx12\"`) or Vulkan, and update to the latest GPU driver."
        ));
    }
    renderer.prepare_headless_render_target()?;

    let mut scene = BaselineScene::load(&mut renderer, Path::new("assets/scenes/quick_save.json"))?;
    let mut accumulator = GpuTimingAccumulator::default();
    let camera2d = Camera2D::new(1.2);
    let viewport = RenderViewport { origin: (0.0, 0.0), size: (1280.0, 720.0) };
    let mesh_camera = Camera3D::new(Vec3::new(6.0, 6.0, 10.0), Vec3::ZERO, 60f32.to_radians(), 0.1, 100.0);
    let mut frames_recorded = 0usize;
    for _ in 0..(args.frames * 2) {
        let sprite_sampler = scene.sprite_sampler_arc();
        scene.step(1.0 / 60.0);
        let (instances, batches) = scene.build_sprite_batches()?;
        let mesh_draws = scene.build_mesh_draws(&mut renderer)?;
        let frame = renderer.render_frame(
            &instances,
            &batches,
            sprite_sampler.as_ref(),
            camera2d.view_projection(physical_size(viewport)),
            viewport,
            &mesh_draws,
            Some(&mesh_camera),
        )?;
        frame.present();
        let timings = renderer.take_gpu_timings();
        if !timings.is_empty() {
            accumulator.record_frame(&timings);
            frames_recorded += 1;
            if frames_recorded >= args.frames {
                break;
            }
        }
    }
    if frames_recorded == 0 {
        return Err(anyhow!("GPU timings unavailable; ensure profiling is supported on this adapter"));
    }

    let snapshot = accumulator.snapshot(
        "gpu_baseline",
        format!("{}", SystemTime::now().duration_since(UNIX_EPOCH)?.as_secs()),
        current_git_commit().unwrap_or_else(|_| "unknown".into()),
    );
    snapshot.write_to_path(&args.output)?;
    if let Some(path) = args.baseline {
        let baseline = GpuBaselineSnapshot::load(&path)?;
        let deltas =
            compare_baselines(&baseline, &snapshot, &args.pass_tolerances, args.default_tolerance_ms)?;
        let regressions: Vec<_> = deltas.iter().filter(|d| !d.within_tolerance).collect();
        if !regressions.is_empty() {
            eprintln!("GPU baseline regressions:");
            for entry in regressions {
                eprintln!(
                    "  {}: current {:.3} ms (baseline {:.3} ms) drift {:.3} ms (limit {:.3} ms)",
                    entry.label,
                    entry.current_avg_ms,
                    entry.baseline_avg_ms,
                    entry.delta_ms,
                    entry.allowed_drift_ms
                );
            }
            return Err(anyhow!("GPU baseline drift exceeded tolerance"));
        }
    }
    Ok(())
}

fn physical_size(viewport: RenderViewport) -> winit::dpi::PhysicalSize<u32> {
    winit::dpi::PhysicalSize::new(viewport.size.0 as u32, viewport.size.1 as u32)
}

struct BaselineScene {
    ecs: EcsWorld,
    assets: AssetManager,
    mesh_registry: MeshRegistry,
    material_registry: MaterialRegistry,
    atlas_views: HashMap<String, Arc<wgpu::TextureView>>,
    sprite_sampler: Arc<wgpu::Sampler>,
}

impl BaselineScene {
    fn load(renderer: &mut Renderer, scene_path: &Path) -> Result<Self> {
        let mut assets = AssetManager::new();
        let device = renderer.device()?;
        let queue = renderer.queue()?;
        assets.set_device(device, queue);

        let mut material_registry = MaterialRegistry::new();
        let mut mesh_registry = MeshRegistry::new(&mut material_registry);
        let mut environment_registry = EnvironmentRegistry::new();
        environment_registry.load_directory("assets/environments")?;

        let scene = Scene::load_from_path(scene_path)?;
        Self::load_scene_dependencies(
            &scene,
            &mut assets,
            &mut mesh_registry,
            &mut material_registry,
            &mut environment_registry,
        )?;
        let mut ecs = EcsWorld::new();
        ecs.load_scene_with_dependencies(
            &scene,
            &assets,
            |key, path| mesh_registry.ensure_mesh(key, path, &mut material_registry),
            |_, _| Ok(()),
            |key, path| environment_registry.retain(key, path),
        )?;

        let sampler = Arc::new(assets.default_sampler().clone());
        let default_atlas = scene
            .dependencies
            .atlas_dependencies()
            .next()
            .map(|dep| dep.key().to_string())
            .unwrap_or_else(|| "main".to_string());
        let atlas_view = assets.atlas_texture_view(&default_atlas)?;
        renderer.init_sprite_pipeline_with_atlas(atlas_view, sampler.as_ref().clone())?;

        let env_key = scene
            .dependencies
            .environment_dependency()
            .map(|dep| dep.key().to_string())
            .unwrap_or_else(|| environment_registry.default_key().to_string());
        let env_gpu = environment_registry.ensure_gpu(&env_key, renderer)?;
        renderer.set_environment(env_gpu.as_ref(), 1.0)?;

        Ok(Self {
            ecs,
            assets,
            mesh_registry,
            material_registry,
            atlas_views: HashMap::new(),
            sprite_sampler: sampler,
        })
    }

    fn load_scene_dependencies(
        scene: &Scene,
        assets: &mut AssetManager,
        mesh_registry: &mut MeshRegistry,
        material_registry: &mut MaterialRegistry,
        environment_registry: &mut EnvironmentRegistry,
    ) -> Result<()> {
        for dep in scene.dependencies.atlas_dependencies() {
            assets
                .retain_atlas(dep.key(), dep.path())
                .with_context(|| format!("Failed to retain atlas '{}'", dep.key()))?;
        }
        for dep in scene.dependencies.clip_dependencies() {
            assets
                .retain_clip(dep.key(), dep.path())
                .with_context(|| format!("Failed to retain clip '{}'", dep.key()))?;
        }
        for dep in scene.dependencies.mesh_dependencies() {
            mesh_registry
                .ensure_mesh(dep.key(), dep.path(), material_registry)
                .with_context(|| format!("Failed to prepare mesh '{}'", dep.key()))?;
        }
        for dep in scene.dependencies.material_dependencies() {
            material_registry
                .retain(dep.key())
                .with_context(|| format!("Failed to retain material '{}'", dep.key()))?;
        }
        for dep in scene.dependencies.environment_dependencies() {
            environment_registry
                .retain(dep.key(), dep.path())
                .with_context(|| format!("Failed to retain environment '{}'", dep.key()))?;
        }
        Ok(())
    }

    fn step(&mut self, dt: f32) {
        self.ecs.update(dt);
    }

    fn sprite_sampler_arc(&self) -> Arc<wgpu::Sampler> {
        self.sprite_sampler.clone()
    }

    fn build_sprite_batches(&mut self) -> Result<(Vec<InstanceData>, Vec<SpriteBatch>)> {
        let sprites = self.ecs.collect_sprite_instances(&self.assets)?;
        let mut grouped: HashMap<Arc<str>, Vec<InstanceData>> = HashMap::new();
        for sprite in sprites {
            let (atlas, data) = sprite.into_gpu();
            grouped.entry(atlas).or_default().push(data);
        }
        let mut instances = Vec::new();
        let mut batches = Vec::new();
        let mut atlas_keys: Vec<_> = grouped.keys().cloned().collect();
        atlas_keys.sort_by(|a, b| a.as_ref().cmp(b.as_ref()));
        for atlas in atlas_keys {
            if let Some(batch_instances) = grouped.remove(&atlas) {
                if batch_instances.is_empty() {
                    continue;
                }
                let start = instances.len();
                instances.extend(batch_instances);
                let end = instances.len();
                let view = self.atlas_view(atlas.as_ref())?;
                batches.push(SpriteBatch { atlas, range: start as u32..end as u32, view });
            }
        }
        Ok((instances, batches))
    }

    fn build_mesh_draws(&mut self, renderer: &mut Renderer) -> Result<Vec<MeshDraw<'_>>> {
        let mesh_instances = self.ecs.collect_mesh_instances();
        for instance in &mesh_instances {
            self.mesh_registry.ensure_gpu(&instance.key, renderer)?;
        }
        let mut draws = Vec::new();
        for instance in mesh_instances {
            let gpu_mesh = self
                .mesh_registry
                .gpu_mesh(&instance.key)
                .ok_or_else(|| anyhow!("GPU mesh '{}' missing", instance.key.clone()))?;
            let material_key =
                instance.material.clone().unwrap_or_else(|| self.material_registry.default_key().to_string());
            self.material_registry.retain(&material_key)?;
            let material_gpu = self.material_registry.prepare_material_gpu(&material_key, renderer)?;
            draws.push(MeshDraw {
                mesh: gpu_mesh,
                model: instance.model,
                lighting: instance.lighting.clone(),
                material: material_gpu,
                casts_shadows: instance.lighting.cast_shadows,
                skin_palette: instance.skin.as_ref().map(|skin| skin.palette.clone()),
            });
        }
        Ok(draws)
    }

    fn atlas_view(&mut self, key: &str) -> Result<Arc<wgpu::TextureView>> {
        if let Some(view) = self.atlas_views.get(key) {
            return Ok(view.clone());
        }
        let view = Arc::new(self.assets.atlas_texture_view(key)?);
        self.atlas_views.insert(key.to_string(), view.clone());
        Ok(view)
    }
}

fn current_git_commit() -> Result<String> {
    use std::process::Command;
    let output = Command::new("git").args(["rev-parse", "HEAD"]).output()?;
    if !output.status.success() {
        return Err(anyhow!("git rev-parse failed"));
    }
    Ok(String::from_utf8_lossy(&output.stdout).trim().to_string())
}
