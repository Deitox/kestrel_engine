use glam::Vec3;

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
}

impl Mesh {
    pub fn new(vertices: Vec<MeshVertex>, indices: Vec<u32>) -> Self {
        Self { vertices, indices }
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
            indices.extend_from_slice(&[
                base,
                base + 1,
                base + 2,
                base,
                base + 2,
                base + 3,
            ]);
        }

        Self { vertices, indices }
    }
}
