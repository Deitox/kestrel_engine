use kestrel_engine::config::WindowConfig;
use kestrel_engine::environment::EnvironmentRegistry;
use kestrel_engine::renderer::Renderer;

#[test]
fn renderer_binds_environment_textures() {
    let window_config = WindowConfig {
        title: "Headless".to_string(),
        width: 64,
        height: 64,
        vsync: false,
        fullscreen: false,
    };
    let mut renderer = pollster::block_on(Renderer::new(&window_config));
    pollster::block_on(renderer.init_headless_for_test()).expect("headless init");

    let mut environment_registry = EnvironmentRegistry::new();
    let default_key = environment_registry.default_key().to_string();
    let env_gpu =
        environment_registry.ensure_gpu(&default_key, &mut renderer).expect("upload default environment");

    renderer.set_environment(&env_gpu, 1.25).expect("bind environment");
    renderer.set_environment_intensity(0.5);
    let (mip_count, intensity) = renderer.environment_parameters().expect("environment parameters");
    assert_eq!(mip_count, env_gpu.specular_mip_count());
    assert!((intensity - 0.5).abs() < f32::EPSILON);
}
