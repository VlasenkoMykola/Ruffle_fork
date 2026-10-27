use crate::descriptors::Descriptors;
use crate::{PosUvVertex, Transforms};
use std::cell::{Cell, RefCell};
use std::mem;

const ESTIMATED_OBJECTS_PER_CHUNK: u64 = 200;

/// How many chunks of draws a frame gives buffers of their own; any more reuse them from
/// the first. Each takes 32 KiB.
const MAX_CHUNKS_PER_FRAME: usize = 256;

pub fn transforms_per_draw(limits: &wgpu::Limits) -> u64 {
    let maximum = limits.max_uniform_buffer_binding_size / size_of::<Transforms>() as u64;
    maximum.min(ESTIMATED_OBJECTS_PER_CHUNK)
}

/// The buffers that one chunk of draws reads its transforms and its bitmaps' quads from.
#[derive(Clone)]
pub struct DynamicTransforms {
    pub buffer: wgpu::Buffer,
    pub vertex_buffer: wgpu::Buffer,
    pub bind_group: wgpu::BindGroup,
}

impl DynamicTransforms {
    fn new(descriptors: &Descriptors, size: u64, vertices_size: u64) -> Self {
        let buffer = descriptors.device.create_buffer(&wgpu::BufferDescriptor {
            label: None,
            size,
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        let vertex_buffer = descriptors.device.create_buffer(&wgpu::BufferDescriptor {
            label: None,
            size: vertices_size,
            usage: wgpu::BufferUsages::VERTEX | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        let bind_group = descriptors
            .device
            .create_bind_group(&wgpu::BindGroupDescriptor {
                label: None,
                layout: &descriptors.bind_layouts.transforms,
                entries: &[wgpu::BindGroupEntry {
                    binding: 0,
                    resource: buffer.as_entire_binding(),
                }],
            });
        Self {
            buffer,
            bind_group,
            vertex_buffer,
        }
    }
}

/// Gives each chunk of draws in a frame buffers of its own, so that no chunk overwrites
/// the transforms of an earlier one while its draws may still be pending. Rewriting one
/// uniform buffer between draws to the same target is what the OpenGL ES drivers of
/// tile-based GPUs, like Android's, defer and get wrong most easily; elements of busy
/// frames showed in other places on Android, and only there.
pub struct DynamicTransformsPool {
    chunks: RefCell<Vec<DynamicTransforms>>,
    next: Cell<usize>,
    transforms_size: u64,
    vertices_size: u64,
}

impl DynamicTransformsPool {
    pub fn new(descriptors: &Descriptors) -> Self {
        Self {
            chunks: RefCell::new(Vec::new()),
            next: Cell::new(0),
            transforms_size: transforms_per_draw(&descriptors.limits)
                * size_of::<Transforms>() as u64,
            vertices_size: (mem::size_of::<PosUvVertex>() as u64 * 4 * ESTIMATED_OBJECTS_PER_CHUNK)
                .min(descriptors.limits.max_buffer_size),
        }
    }

    /// The size of each chunk's transforms buffer.
    pub fn transforms_size(&self) -> u64 {
        self.transforms_size
    }

    /// The size of each chunk's vertex buffer.
    pub fn vertices_size(&self) -> u64 {
        self.vertices_size
    }

    /// The buffers for the next chunk of draws.
    pub fn next(&self, descriptors: &Descriptors) -> DynamicTransforms {
        let index = self.next.get();
        self.next.set((index + 1) % MAX_CHUNKS_PER_FRAME);
        let mut chunks = self.chunks.borrow_mut();
        if index == chunks.len() {
            chunks.push(DynamicTransforms::new(
                descriptors,
                self.transforms_size,
                self.vertices_size,
            ));
        }
        chunks[index].clone()
    }

    /// Starts handing out the buffers from the first again, once the frame that used them
    /// is submitted.
    pub fn frame_submitted(&self) {
        self.next.set(0);
    }
}
