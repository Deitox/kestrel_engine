use anyhow::{anyhow, bail, Context, Result};
use glam::{Vec2, Vec3, Vec4};
use gltf::mesh::Mode;
use std::collections::HashMap;
use std::path::Path;

#[repr(C)]
#[derive(Clone, Copy, Debug, bytemuck::Pod, bytemuck::Zeroable)]
pub struct MeshVertex {
    pub position: [f32; 3],
    pub normal: [f32; 3],
    pub tangent: [f32; 4],
    pub uv: [f32; 2],
    pub joints: [u16; 4],
    pub weights: [f32; 4],
}

impl MeshVertex {
    pub fn new(position: Vec3, normal: Vec3, tangent: Vec4, uv: Vec2) -> Self {
        Self {
            position: position.to_array(),
            normal: normal.to_array(),
            tangent: tangent.to_array(),
            uv: uv.to_array(),
            joints: [0; 4],
            weights: [0.0; 4],
        }
    }

    pub fn with_skin(mut self, joints: [u16; 4], weights: [f32; 4]) -> Self {
        self.joints = joints;
        self.weights = weights;
        self
    }

    pub fn layout<'a>() -> wgpu::VertexBufferLayout<'a> {
        use std::mem;
        wgpu::VertexBufferLayout {
            array_stride: mem::size_of::<MeshVertex>() as wgpu::BufferAddress,
            step_mode: wgpu::VertexStepMode::Vertex,
            attributes: &[
                wgpu::VertexAttribute {
                    offset: 0,
                    shader_location: 0,
                    format: wgpu::VertexFormat::Float32x3,
                },
                wgpu::VertexAttribute {
                    offset: 12,
                    shader_location: 1,
                    format: wgpu::VertexFormat::Float32x3,
                },
                wgpu::VertexAttribute {
                    offset: 24,
                    shader_location: 2,
                    format: wgpu::VertexFormat::Float32x4,
                },
                wgpu::VertexAttribute {
                    offset: 40,
                    shader_location: 3,
                    format: wgpu::VertexFormat::Float32x2,
                },
                wgpu::VertexAttribute {
                    offset: 48,
                    shader_location: 4,
                    format: wgpu::VertexFormat::Uint16x4,
                },
                wgpu::VertexAttribute {
                    offset: 56,
                    shader_location: 5,
                    format: wgpu::VertexFormat::Float32x4,
                },
            ],
        }
    }
}

#[derive(Clone, Debug)]
pub struct Mesh {
    pub vertices: Vec<MeshVertex>,
    pub indices: Vec<u32>,
    pub subsets: Vec<MeshSubset>,
    pub bounds: MeshBounds,
}

#[derive(Clone, Debug)]
pub struct MeshBounds {
    pub min: Vec3,
    pub max: Vec3,
    pub center: Vec3,
    pub radius: f32,
}

#[derive(Clone, Debug)]
pub struct MeshSubset {
    pub name: Option<String>,
    pub index_offset: u32,
    pub index_count: u32,
    pub material: Option<String>,
}

#[derive(Clone, Debug)]
pub struct ImportedTexture {
    pub key: String,
    pub width: u32,
    pub height: u32,
    pub data: Vec<u8>,
}

#[derive(Clone, Debug)]
pub struct MaterialTextureBinding {
    pub texture_key: String,
    pub tex_coord: u32,
    pub srgb: bool,
    pub scale: f32,
}

#[derive(Clone, Debug)]
pub struct ImportedMaterial {
    pub key: String,
    pub label: String,
    pub base_color_factor: [f32; 4],
    pub metallic_factor: f32,
    pub roughness_factor: f32,
    pub emissive_factor: [f32; 3],
    pub base_color_texture: Option<MaterialTextureBinding>,
    pub metallic_roughness_texture: Option<MaterialTextureBinding>,
    pub normal_texture: Option<MaterialTextureBinding>,
    pub emissive_texture: Option<MaterialTextureBinding>,
    pub source: Option<String>,
}

#[derive(Clone, Debug)]
pub struct MeshImport {
    pub mesh: Mesh,
    pub materials: Vec<ImportedMaterial>,
    pub textures: Vec<ImportedTexture>,
}

impl Mesh {
    pub fn new(vertices: Vec<MeshVertex>, indices: Vec<u32>) -> Self {
        let subset =
            MeshSubset { name: None, index_offset: 0, index_count: indices.len() as u32, material: None };
        let bounds = MeshBounds::from_vertices(&vertices);
        Self { vertices, indices, subsets: vec![subset], bounds }
    }

    pub fn cube(size: f32) -> Self {
        let hs = size * 0.5;
        let positions = [
            Vec3::new(-hs, -hs, -hs),
            Vec3::new(hs, -hs, -hs),
            Vec3::new(hs, hs, -hs),
            Vec3::new(-hs, hs, -hs),
            Vec3::new(-hs, -hs, hs),
            Vec3::new(hs, -hs, hs),
            Vec3::new(hs, hs, hs),
            Vec3::new(-hs, hs, hs),
        ];
        let normals = [
            Vec3::new(0.0, 0.0, -1.0),
            Vec3::new(0.0, 0.0, 1.0),
            Vec3::new(-1.0, 0.0, 0.0),
            Vec3::new(1.0, 0.0, 0.0),
            Vec3::new(0.0, 1.0, 0.0),
            Vec3::new(0.0, -1.0, 0.0),
        ];

        let uv_quad = [Vec2::new(0.0, 0.0), Vec2::new(1.0, 0.0), Vec2::new(1.0, 1.0), Vec2::new(0.0, 1.0)];
        let mut vertices = Vec::with_capacity(24);
        let mut write_face = |indices: [usize; 4], normal: Vec3| {
            for (i, &index) in indices.iter().enumerate() {
                vertices.push(MeshVertex::new(
                    positions[index],
                    normal,
                    Vec4::new(1.0, 0.0, 0.0, 1.0),
                    uv_quad[i],
                ));
            }
        };

        write_face([0, 3, 2, 1], normals[0]); // back
        write_face([4, 5, 6, 7], normals[1]); // front
        write_face([0, 4, 7, 3], normals[2]); // left
        write_face([1, 2, 6, 5], normals[3]); // right
        write_face([3, 7, 6, 2], normals[4]); // top
        write_face([0, 1, 5, 4], normals[5]); // bottom

        let mut indices = Vec::with_capacity(36);
        for face in 0..6 {
            let base = face * 4;
            indices.extend_from_slice(&[base, base + 1, base + 2, base, base + 2, base + 3]);
        }

        compute_tangents(&mut vertices, &indices);

        Self::new(vertices, indices)
    }

    pub fn load_gltf_with_materials(path: impl AsRef<Path>) -> Result<MeshImport> {
        let path_ref = path.as_ref();
        let (document, buffers, images) = gltf::import(path_ref)
            .with_context(|| format!("Failed to import glTF from {}", path_ref.display()))?;
        let mesh =
            document.meshes().next().ok_or_else(|| anyhow!("No meshes found in {}", path_ref.display()))?;

        let mut textures = Vec::new();
        let mut texture_key_map: HashMap<usize, String> = HashMap::new();
        for texture in document.textures() {
            let source = texture.source();
            let image_data = images
                .get(source.index())
                .ok_or_else(|| anyhow!("Image index {} missing in {}", source.index(), path_ref.display()))?;
            let pixels = convert_image_to_rgba(image_data)?;
            let key = format!("{}::tex{}", path_ref.display(), texture.index());
            textures.push(ImportedTexture {
                key: key.clone(),
                width: image_data.width,
                height: image_data.height,
                data: pixels,
            });
            texture_key_map.insert(texture.index(), key);
        }

        let default_material_key = format!("{}::default", path_ref.display());
        let mut materials = Vec::new();
        let mut material_key_map: HashMap<usize, String> = HashMap::new();
        for (mat_index, material) in document.materials().enumerate() {
            let label =
                material.name().map(|s| s.to_string()).unwrap_or_else(|| format!("material_{mat_index}"));
            let key = format!("{}::{}", path_ref.display(), label);
            let actual_index = material.index().unwrap_or(mat_index);
            material_key_map.insert(actual_index, key.clone());

            let pbr = material.pbr_metallic_roughness();
            let base_color_factor = pbr.base_color_factor();
            let emissive_factor = material.emissive_factor();
            let metallic_factor = pbr.metallic_factor();
            let roughness_factor = pbr.roughness_factor();

            let base_color_texture = pbr.base_color_texture().and_then(|info| {
                let tex = info.texture();
                texture_key_map.get(&tex.index()).map(|key_str| MaterialTextureBinding {
                    texture_key: key_str.clone(),
                    tex_coord: info.tex_coord(),
                    srgb: true,
                    scale: 1.0,
                })
            });
            let metallic_roughness_texture = pbr.metallic_roughness_texture().and_then(|info| {
                let tex = info.texture();
                texture_key_map.get(&tex.index()).map(|key_str| MaterialTextureBinding {
                    texture_key: key_str.clone(),
                    tex_coord: info.tex_coord(),
                    srgb: false,
                    scale: 1.0,
                })
            });
            let normal_texture = material.normal_texture().and_then(|info| {
                let tex = info.texture();
                texture_key_map.get(&tex.index()).map(|key_str| MaterialTextureBinding {
                    texture_key: key_str.clone(),
                    tex_coord: info.tex_coord(),
                    srgb: false,
                    scale: info.scale(),
                })
            });
            let emissive_texture = material.emissive_texture().and_then(|info| {
                let tex = info.texture();
                texture_key_map.get(&tex.index()).map(|key_str| MaterialTextureBinding {
                    texture_key: key_str.clone(),
                    tex_coord: info.tex_coord(),
                    srgb: true,
                    scale: 1.0,
                })
            });

            materials.push(ImportedMaterial {
                key,
                label,
                base_color_factor,
                metallic_factor,
                roughness_factor,
                emissive_factor,
                base_color_texture,
                metallic_roughness_texture,
                normal_texture,
                emissive_texture,
                source: Some(path_ref.display().to_string()),
            });
        }

        if !materials.iter().any(|mat| mat.key == default_material_key) {
            materials.push(ImportedMaterial {
                key: default_material_key.clone(),
                label: "Default".to_string(),
                base_color_factor: [1.0, 1.0, 1.0, 1.0],
                metallic_factor: 0.0,
                roughness_factor: 1.0,
                emissive_factor: [0.0, 0.0, 0.0],
                base_color_texture: None,
                metallic_roughness_texture: None,
                normal_texture: None,
                emissive_texture: None,
                source: None,
            });
        }

        let mut vertices: Vec<MeshVertex> = Vec::new();
        let mut indices: Vec<u32> = Vec::new();
        let mut subsets: Vec<MeshSubset> = Vec::new();

        for (primitive_index, primitive) in mesh.primitives().enumerate() {
            if primitive.mode() != Mode::Triangles {
                continue;
            }
            let reader = primitive.reader(|buffer| Some(&buffers[buffer.index()]));
            let positions_iter = reader
                .read_positions()
                .ok_or_else(|| anyhow!("POSITION attribute missing in {}", path_ref.display()))?;
            let positions: Vec<Vec3> = positions_iter.map(Vec3::from_array).collect();
            if positions.is_empty() {
                continue;
            }

            let mut normals: Vec<Vec3> = reader
                .read_normals()
                .map(|it| it.map(Vec3::from_array).collect())
                .unwrap_or_else(|| vec![Vec3::ZERO; positions.len()]);

            let mut tex_coords: Vec<Vec2> = reader
                .read_tex_coords(0)
                .map(|coords| coords.into_f32().map(Vec2::from_array).collect())
                .unwrap_or_else(|| vec![Vec2::ZERO; positions.len()]);

            let mut joints: Vec<[u16; 4]> = reader
                .read_joints(0)
                .map(|it| it.into_u16().collect())
                .unwrap_or_else(|| vec![[0; 4]; positions.len()]);
            let mut weights: Vec<[f32; 4]> = reader
                .read_weights(0)
                .map(|it| it.into_f32().collect())
                .unwrap_or_else(|| vec![[0.0; 4]; positions.len()]);

            let local_indices: Vec<u32> = reader
                .read_indices()
                .map(|read| read.into_u32().collect())
                .unwrap_or_else(|| (0..positions.len() as u32).collect());

            if normals.is_empty()
                || normals.len() != positions.len()
                || normals.iter().all(|n| n.length_squared() == 0.0)
            {
                normals = compute_normals(&positions, &local_indices);
            }

            if tex_coords.len() != positions.len() {
                tex_coords.resize(positions.len(), Vec2::ZERO);
            }
            if joints.len() != positions.len() {
                joints.resize(positions.len(), [0; 4]);
            }
            if weights.len() != positions.len() {
                weights.resize(positions.len(), [0.0; 4]);
            }

            let base_vertex = vertices.len() as u32;
            vertices.extend(positions.iter().enumerate().map(|(i, pos)| {
                let norm = normals.get(i).copied().unwrap_or(Vec3::Y).normalize_or_zero();
                let uv = tex_coords.get(i).copied().unwrap_or(Vec2::ZERO);
                let joint_indices = joints.get(i).copied().unwrap_or([0; 4]);
                let weight_values = weights.get(i).copied().unwrap_or([0.0; 4]);
                MeshVertex::new(*pos, norm, Vec4::new(1.0, 0.0, 0.0, 1.0), uv)
                    .with_skin(joint_indices, weight_values)
            }));

            let index_offset = indices.len() as u32;
            indices.extend(local_indices.iter().map(|idx| idx + base_vertex));
            let index_count = (indices.len() as u32) - index_offset;
            let material_key = primitive
                .material()
                .index()
                .and_then(|idx| material_key_map.get(&idx).cloned())
                .unwrap_or_else(|| default_material_key.clone());
            let name = mesh
                .name()
                .map(|mesh_name| format!("{}::{}", mesh_name, primitive_index))
                .or_else(|| Some(format!("primitive_{primitive_index}")));
            subsets.push(MeshSubset { name, index_offset, index_count, material: Some(material_key) });
        }

        compute_tangents(&mut vertices, &indices);

        if subsets.is_empty() {
            return Err(anyhow!("Mesh in {} contains no triangle primitives", path_ref.display()));
        }

        let bounds = MeshBounds::from_vertices(&vertices);

        let mesh = Mesh { vertices, indices, subsets, bounds };

        Ok(MeshImport { mesh, materials, textures })
    }
    pub fn load_gltf(path: impl AsRef<Path>) -> Result<Self> {
        Ok(Self::load_gltf_with_materials(path)?.mesh)
    }
}

fn convert_image_to_rgba(image: &gltf::image::Data) -> Result<Vec<u8>> {
    match image.format {
        gltf::image::Format::R8 => {
            let mut out = Vec::with_capacity(image.pixels.len() * 4);
            for &value in &image.pixels {
                out.extend_from_slice(&[value, value, value, 255]);
            }
            Ok(out)
        }
        gltf::image::Format::R8G8 => {
            let mut out = Vec::with_capacity(image.pixels.len() / 2 * 4);
            for chunk in image.pixels.chunks_exact(2) {
                out.extend_from_slice(&[chunk[0], chunk[1], 0, 255]);
            }
            Ok(out)
        }
        gltf::image::Format::R8G8B8 => {
            let mut out = Vec::with_capacity(image.pixels.len() / 3 * 4);
            for chunk in image.pixels.chunks_exact(3) {
                out.extend_from_slice(&[chunk[0], chunk[1], chunk[2], 255]);
            }
            Ok(out)
        }
        gltf::image::Format::R8G8B8A8 => Ok(image.pixels.clone()),
        other => bail!("Unsupported image format {:?}", other),
    }
}

fn compute_normals(positions: &[Vec3], indices: &[u32]) -> Vec<Vec3> {
    let mut normals = vec![Vec3::ZERO; positions.len()];
    for tri in indices.chunks(3) {
        if tri.len() < 3 {
            continue;
        }
        let i0 = tri[0] as usize;
        let i1 = tri[1] as usize;
        let i2 = tri[2] as usize;
        if i0 >= positions.len() || i1 >= positions.len() || i2 >= positions.len() {
            continue;
        }
        let a = positions[i0];
        let b = positions[i1];
        let c = positions[i2];
        let normal = (b - a).cross(c - a);
        if normal.length_squared() > 0.0 {
            normals[i0] += normal;
            normals[i1] += normal;
            normals[i2] += normal;
        }
    }
    for normal in &mut normals {
        if normal.length_squared() > 0.0 {
            *normal = normal.normalize();
        } else {
            *normal = Vec3::Y;
        }
    }
    normals
}

fn compute_tangents(vertices: &mut [MeshVertex], indices: &[u32]) {
    if vertices.is_empty() || indices.is_empty() {
        return;
    }
    let mut tan1 = vec![Vec3::ZERO; vertices.len()];
    let mut tan2 = vec![Vec3::ZERO; vertices.len()];

    for tri in indices.chunks(3) {
        if tri.len() < 3 {
            continue;
        }
        let i0 = tri[0] as usize;
        let i1 = tri[1] as usize;
        let i2 = tri[2] as usize;
        if i0 >= vertices.len() || i1 >= vertices.len() || i2 >= vertices.len() {
            continue;
        }

        let v0 = Vec3::from_array(vertices[i0].position);
        let v1 = Vec3::from_array(vertices[i1].position);
        let v2 = Vec3::from_array(vertices[i2].position);

        let uv0 = Vec2::from_array(vertices[i0].uv);
        let uv1 = Vec2::from_array(vertices[i1].uv);
        let uv2 = Vec2::from_array(vertices[i2].uv);

        let delta_pos1 = v1 - v0;
        let delta_pos2 = v2 - v0;
        let delta_uv1 = uv1 - uv0;
        let delta_uv2 = uv2 - uv0;

        let denom = delta_uv1.x * delta_uv2.y - delta_uv1.y * delta_uv2.x;
        if denom.abs() < 1e-8 {
            continue;
        }
        let r = 1.0 / denom;
        let sdir = (delta_pos1 * delta_uv2.y - delta_pos2 * delta_uv1.y) * r;
        let tdir = (delta_pos2 * delta_uv1.x - delta_pos1 * delta_uv2.x) * r;

        tan1[i0] += sdir;
        tan1[i1] += sdir;
        tan1[i2] += sdir;

        tan2[i0] += tdir;
        tan2[i1] += tdir;
        tan2[i2] += tdir;
    }

    for (i, vertex) in vertices.iter_mut().enumerate() {
        let normal = Vec3::from_array(vertex.normal);
        let t1 = tan1[i];
        if t1.length_squared() > 0.0 {
            let tangent = (t1 - normal * normal.dot(t1)).normalize_or_zero();
            let bitangent = tan2[i];
            let w = if normal.cross(t1).dot(bitangent) < 0.0 { -1.0 } else { 1.0 };
            vertex.tangent = [tangent.x, tangent.y, tangent.z, w];
        } else {
            let tangent = Vec3::X;
            vertex.tangent = [tangent.x, tangent.y, tangent.z, 1.0];
        }
    }
}

impl MeshBounds {
    pub fn from_vertices(vertices: &[MeshVertex]) -> Self {
        let mut min = Vec3::splat(f32::INFINITY);
        let mut max = Vec3::splat(f32::NEG_INFINITY);
        for vertex in vertices {
            let pos = Vec3::from_array(vertex.position);
            min = min.min(pos);
            max = max.max(pos);
        }
        if vertices.is_empty() {
            return MeshBounds { min: Vec3::ZERO, max: Vec3::ZERO, center: Vec3::ZERO, radius: 0.0 };
        }
        let center = (min + max) * 0.5;
        let mut radius: f32 = 0.0;
        for vertex in vertices {
            let pos = Vec3::from_array(vertex.position);
            radius = radius.max((pos - center).length());
        }
        MeshBounds { min, max, center, radius }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn load_demo_gltf_mesh() {
        let mesh = Mesh::load_gltf("assets/models/demo_triangle.gltf").expect("demo gltf should load");
        assert_eq!(mesh.vertices.len(), 3);
        assert_eq!(mesh.indices, vec![0, 1, 2]);
        assert_eq!(mesh.subsets.len(), 1);
        assert_eq!(mesh.subsets[0].index_offset, 0);
        assert_eq!(mesh.subsets[0].index_count, 3);
        for vertex in &mesh.vertices {
            let normal = Vec3::from_array(vertex.normal);
            assert!((normal - Vec3::Z).length_squared() < 1e-4);
            let uv = Vec2::from_array(vertex.uv);
            assert!(uv.x >= 0.0 && uv.x <= 1.0);
            assert!(uv.y >= 0.0 && uv.y <= 1.0);
            let tangent = Vec3::new(vertex.tangent[0], vertex.tangent[1], vertex.tangent[2]);
            assert!(tangent.length_squared() > 0.0);
        }
    }
}
