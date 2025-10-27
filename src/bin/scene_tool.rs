use anyhow::{anyhow, Context, Result};
use kestrel_engine::scene::{Scene, SceneEntityId};
use std::collections::HashSet;
use std::env;
use std::path::Path;
use std::process;

fn main() {
    if let Err(err) = run() {
        eprintln!("error: {err:?}");
        process::exit(1);
    }
}

fn run() -> Result<()> {
    let mut args = env::args().skip(1);
    let Some(command) = args.next() else {
        print_usage();
        return Ok(());
    };
    match command.as_str() {
        "validate" => {
            let scene_path = args
                .next()
                .ok_or_else(|| anyhow!("validate requires a path: scene_tool validate <scene>"))?;
            cmd_validate(&scene_path)
        }
        "list" => {
            let scene_path =
                args.next().ok_or_else(|| anyhow!("list requires a path: scene_tool list <scene>"))?;
            cmd_list(&scene_path)
        }
        "extract" => {
            let scene_path = args.next().ok_or_else(|| {
                anyhow!("extract requires arguments: scene_tool extract <scene> <entity_id> <output>")
            })?;
            let entity_id = args.next().ok_or_else(|| anyhow!("extract missing entity id argument"))?;
            let output_path = args.next().ok_or_else(|| anyhow!("extract missing output path argument"))?;
            cmd_extract(&scene_path, &entity_id, &output_path)
        }
        "convert" => {
            let input = args
                .next()
                .ok_or_else(|| anyhow!("convert requires input path: scene_tool convert <in> <out>"))?;
            let output = args
                .next()
                .ok_or_else(|| anyhow!("convert requires output path: scene_tool convert <in> <out>"))?;
            cmd_convert(&input, &output)
        }
        "help" | "--help" | "-h" => {
            print_usage();
            Ok(())
        }
        other => Err(anyhow!("unknown command '{other}'")),
    }
}

fn print_usage() {
    eprintln!(
        "Scene Tool

Usage:
  scene_tool validate <scene_path>     Validate entity IDs and dependencies
  scene_tool list <scene_path>         List entity IDs, parents, and optional names
  scene_tool extract <scene> <id> <out>  Extract a subtree by entity ID into a new scene
  scene_tool convert <input> <output>  Convert between JSON (.json) and binary (.kscene) scenes
  scene_tool help                      Show this message
"
    );
}

fn cmd_validate(scene_path: &str) -> Result<()> {
    let scene = load_scene(scene_path)?;
    let mut ids = HashSet::with_capacity(scene.entities.len());
    let mut issues = Vec::new();

    for entity in &scene.entities {
        if !ids.insert(entity.id.as_str().to_string()) {
            issues.push(format!("duplicate entity id '{}'", entity.id.as_str()));
        }
    }

    for entity in &scene.entities {
        if let Some(parent_id) = entity.parent_id.as_ref() {
            if !ids.contains(parent_id.as_str()) {
                issues.push(format!(
                    "entity '{}' references missing parent '{}'",
                    entity.id.as_str(),
                    parent_id.as_str()
                ));
            }
        }
        if let Some(parent_index) = entity.parent {
            if parent_index >= scene.entities.len() {
                issues.push(format!(
                    "entity '{}' has parent index {} outside entity list",
                    entity.id.as_str(),
                    parent_index
                ));
            }
        }
        if let Some(sprite) = &entity.sprite {
            if !scene.dependencies.contains_atlas(&sprite.atlas) {
                issues.push(format!(
                    "entity '{}' uses atlas '{}' that is not recorded in dependencies",
                    entity.id.as_str(),
                    sprite.atlas
                ));
            }
        }
        if let Some(mesh) = &entity.mesh {
            if !scene.dependencies.contains_mesh(&mesh.key) {
                issues.push(format!(
                    "entity '{}' uses mesh '{}' that is not recorded in dependencies",
                    entity.id.as_str(),
                    mesh.key
                ));
            }
            if let Some(material) = &mesh.material {
                if !scene.dependencies.contains_material(material) {
                    issues.push(format!(
                        "entity '{}' references material '{}' that is not recorded in dependencies",
                        entity.id.as_str(),
                        material
                    ));
                }
            }
        }
    }

    if let Some(environment) = scene.metadata.environment.as_ref() {
        if !scene.dependencies.contains_environment(&environment.key) {
            issues.push(format!(
                "scene metadata references environment '{}' that is not recorded in dependencies",
                environment.key
            ));
        }
    }

    if issues.is_empty() {
        println!(
            "Scene '{}' is valid. Entities: {}. Atlases: {}  Meshes: {}  Materials: {}",
            scene_path,
            scene.entities.len(),
            scene.dependencies.atlas_dependencies().count(),
            scene.dependencies.mesh_dependencies().count(),
            scene.dependencies.material_dependencies().count(),
        );
        Ok(())
    } else {
        Err(anyhow!(format!("scene '{}' has issues:\n  - {}", scene_path, issues.join("\n  - "))))
    }
}

fn cmd_list(scene_path: &str) -> Result<()> {
    let scene = load_scene(scene_path)?;
    println!("{:<5} {:<38} {:<38} {}", "Idx", "Entity ID", "Parent ID", "Name/Sprite");
    println!("{}", "-".repeat(128));
    for (index, entity) in scene.entities.iter().enumerate() {
        let parent = entity.parent_id.as_ref().map(SceneEntityId::as_str).unwrap_or("-");
        let label = entity
            .name
            .as_deref()
            .or_else(|| entity.sprite.as_ref().map(|sprite| sprite.region.as_str()))
            .unwrap_or("-");
        println!("{:<5} {:<38} {:<38} {}", index, entity.id.as_str(), parent, label);
    }
    Ok(())
}

fn cmd_extract(scene_path: &str, entity_id: &str, output_path: &str) -> Result<()> {
    let scene = load_scene(scene_path)?;
    let Some(mut entities) = scene.clone_subtree(entity_id) else {
        return Err(anyhow!(format!("entity '{}' not found in scene '{}'", entity_id, scene_path)));
    };
    if entities.is_empty() {
        return Err(anyhow!("no entities collected for subtree rooted at '{entity_id}'"));
    }
    let dependencies = scene.dependencies.subset_for_entities(&entities, scene.metadata.environment.as_ref());
    let prefab =
        Scene { metadata: scene.metadata.clone(), dependencies, entities: std::mem::take(&mut entities) };
    prefab.save_to_path(output_path)?;
    println!("Extracted {} entities rooted at '{}' into '{}'", prefab.entities.len(), entity_id, output_path);
    Ok(())
}

fn cmd_convert(input_path: &str, output_path: &str) -> Result<()> {
    let scene = load_scene(input_path)?;
    scene.save_to_path(output_path)?;
    println!("Converted '{}' -> '{}'", input_path, output_path);
    Ok(())
}

fn load_scene(path: &str) -> Result<Scene> {
    let normalized = Path::new(path).canonicalize().unwrap_or_else(|_| Path::new(path).to_path_buf());
    Scene::load_from_path(&normalized).with_context(|| format!("loading scene '{}'", normalized.display()))
}
