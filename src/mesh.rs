use anyhow::{anyhow, Context, Result};
use glam::Vec3;
use gltf::mesh::Mode;
use std::path::Path;

#[repr(C)]
#[derive(Clone, Copy, Debug, bytemuck::Pod, bytemuck::Zeroable)]
pub struct MeshVertex {
    pub position: [f32; 3],
    pub normal: [f32; 3],
}

impl MeshVertex {
    pub fn new(position: Vec3, normal: Vec3) -> Self {
        Self { position: position.to_array(), normal: normal.to_array() }
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

        let mut vertices = Vec::with_capacity(24);
        let mut write_face = |indices: [usize; 4], normal: Vec3| {
            for &index in &indices {
                vertices.push(MeshVertex::new(positions[index], normal));
            }
        };

        write_face([0, 1, 2, 3], normals[0]); // back
        write_face([4, 5, 6, 7], normals[1]); // front
        write_face([0, 3, 7, 4], normals[2]); // left
        write_face([1, 5, 6, 2], normals[3]); // right
        write_face([3, 2, 6, 7], normals[4]); // top
        write_face([0, 1, 5, 4], normals[5]); // bottom

        let mut indices = Vec::with_capacity(36);
        for face in 0..6 {
            let base = face * 4;
            indices.extend_from_slice(&[base, base + 1, base + 2, base, base + 2, base + 3]);
        }

        Self::new(vertices, indices)
    }

    pub fn load_gltf(path: impl AsRef<Path>) -> Result<Self> {
        let path_ref = path.as_ref();
        let (document, buffers, _) = gltf::import(path_ref)
            .with_context(|| format!("Failed to import glTF from {}", path_ref.display()))?;

        let mesh =
            document.meshes().next().ok_or_else(|| anyhow!("No meshes found in {}", path_ref.display()))?;
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

            let base_vertex = vertices.len() as u32;
            vertices.extend(
                positions
                    .iter()
                    .zip(normals.iter())
                    .map(|(pos, norm)| MeshVertex::new(*pos, norm.normalize_or_zero())),
            );

            let index_offset = indices.len() as u32;
            indices.extend(local_indices.iter().map(|idx| idx + base_vertex));
            let index_count = (indices.len() as u32) - index_offset;
            let material = primitive.material().name().map(|s| s.to_string());
            let name = mesh
                .name()
                .map(|mesh_name| format!("{}::{}", mesh_name, primitive_index))
                .or_else(|| Some(format!("primitive_{primitive_index}")));
            subsets.push(MeshSubset { name, index_offset, index_count, material });
        }

        if subsets.is_empty() {
            return Err(anyhow!("Mesh in {} contains no triangle primitives", path_ref.display()));
        }

        let bounds = MeshBounds::from_vertices(&vertices);

        Ok(Mesh { vertices, indices, subsets, bounds })
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
        let normals: Vec<Vec3> = mesh.vertices.iter().map(|v| Vec3::from_array(v.normal)).collect();
        for normal in normals {
            assert!((normal - Vec3::Z).length_squared() < 1e-4);
        }
    }
}
