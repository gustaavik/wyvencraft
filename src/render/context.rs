//! Vulkan device context: device, graphics queue, and the standard allocators
//! shared across the renderer.
//!
//! Borrows what it needs from `vulkano-util`'s `VulkanoContext` (which selects the
//! device and creates the queues) so the app can keep the `VulkanoContext` around
//! for window creation.

use std::sync::Arc;

use vulkano::command_buffer::allocator::StandardCommandBufferAllocator;
use vulkano::descriptor_set::allocator::StandardDescriptorSetAllocator;
use vulkano::device::{Device, Queue};
use vulkano::memory::allocator::StandardMemoryAllocator;
use vulkano_util::context::VulkanoContext;

/// Owns the Vulkan device handle and the allocators every subsystem draws from.
pub struct RenderContext {
    device: Arc<Device>,
    graphics_queue: Arc<Queue>,
    pub memory_allocator: Arc<StandardMemoryAllocator>,
    pub command_allocator: Arc<StandardCommandBufferAllocator>,
    pub descriptor_allocator: Arc<StandardDescriptorSetAllocator>,
}

impl RenderContext {
    /// Build from the app's `VulkanoContext`.
    pub fn from_vulkano(vulkano: &VulkanoContext) -> Arc<Self> {
        let device = vulkano.device().clone();
        let command_allocator = Arc::new(StandardCommandBufferAllocator::new(
            device.clone(),
            Default::default(),
        ));
        let descriptor_allocator = Arc::new(StandardDescriptorSetAllocator::new(
            device.clone(),
            Default::default(),
        ));
        Arc::new(Self {
            graphics_queue: vulkano.graphics_queue().clone(),
            memory_allocator: vulkano.memory_allocator().clone(),
            command_allocator,
            descriptor_allocator,
            device,
        })
    }

    pub fn device(&self) -> &Arc<Device> {
        &self.device
    }

    pub fn graphics_queue(&self) -> &Arc<Queue> {
        &self.graphics_queue
    }
}
