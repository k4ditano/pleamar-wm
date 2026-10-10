//! `pleamar-wm probe`: what a session of its own needs from the card, tried
//! without taking the screen. Buffers made for the monitor (GBM, scanout) are
//! read into wgpu as places to paint in, painted a colour, and read back.

use pleamar::wgpu;
use smithay::reexports::gbm;
use std::os::fd::OwnedFd;

pub fn run() -> Result<(), String> {
    let instance = wgpu::Instance::new(wgpu::InstanceDescriptor { backends: wgpu::Backends::VULKAN, ..wgpu::InstanceDescriptor::new_without_display_handle() });
    let gpu = pleamar::gpu::Gpu::new(&instance, None);
    let modifiers = gpu.render_modifiers().to_vec();
    println!("probe · the card paints BGRA in {} layouts: {:x?}", modifiers.len(), modifiers);
    let node: OwnedFd = std::fs::OpenOptions::new().read(true).write(true).open("/dev/dri/renderD128").map_err(|e| format!("no render node: {e}"))?.into();
    let device = gbm::Device::new(node).map_err(|e| format!("no gbm: {e}"))?;
    let (w, h) = (1920u32, 1080u32);
    // A card that cannot be told a layout: the buffer as its driver lays it out.
    let unlaid = pleamar::dmabuf::without_layouts(gpu.device(), &modifiers);
    let uses = gbm::BufferObjectFlags::SCANOUT | gbm::BufferObjectFlags::RENDERING;
    let made = if unlaid {
        println!("probe · no layouts to name: the buffer as the driver lays it out");
        device.create_buffer_object::<()>(w, h, gbm::Format::Xrgb8888, uses)
    } else {
        device.create_buffer_object_with_modifiers2::<()>(w, h, gbm::Format::Xrgb8888, modifiers.iter().map(|m| gbm::Modifier::from(*m)), uses)
    };
    let bo = made.map_err(|e| format!("gbm made no buffer for the screen: {e}"))?;
    let said: u64 = bo.modifier().into();
    let modifier: u64 = if unlaid { pleamar::dmabuf::NO_LAYOUT } else { said };
    println!("probe · a buffer for the screen: modifier {said:#x}, stride {}", bo.stride_for_plane(0));
    let fd = bo.fd_for_plane(0).map_err(|e| e.to_string())?;
    let texture = pleamar::gpu::Gpu::import_dmabuf(gpu.device(), fd, (w, h), modifier, bo.stride_for_plane(0), bo.offset(0), wgpu::TextureUses::COLOR_TARGET, wgpu::TextureUsages::RENDER_ATTACHMENT, wgpu::TextureUses::UNINITIALIZED)?;
    println!("probe · read into wgpu as a place to paint in");
    let view = texture.create_view(&Default::default());
    let mut encoder = gpu.device().create_command_encoder(&Default::default());
    {
        let _pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
            label: None,
            color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                view: &view,
                depth_slice: None,
                resolve_target: None,
                ops: wgpu::Operations { load: wgpu::LoadOp::Clear(wgpu::Color { r: 0.0, g: 0.5, b: 1.0, a: 1.0 }), store: wgpu::StoreOp::Store },
            })],
            depth_stencil_attachment: None,
            timestamp_writes: None,
            occlusion_query_set: None,
            multiview_mask: None,
        });
    }
    let index = gpu.queue().submit(Some(encoder.finish()));
    let _ = gpu.device().poll(wgpu::PollType::Wait { submission_index: Some(index), timeout: Some(std::time::Duration::from_secs(2)) });
    println!("probe · painted");
    // Read back through a second, separate import of the same memory: what
    // anyone else holding the buffer —the monitor— sees.
    let fd = bo.fd_for_plane(0).map_err(|e| e.to_string())?;
    let other = pleamar::gpu::Gpu::import_dmabuf(gpu.device(), fd, (w, h), modifier, bo.stride_for_plane(0), bo.offset(0), wgpu::TextureUses::COPY_SRC, wgpu::TextureUsages::COPY_SRC, wgpu::TextureUses::COPY_SRC)?;
    let out = gpu.device().create_buffer(&wgpu::BufferDescriptor { label: None, size: 256, usage: wgpu::BufferUsages::COPY_DST | wgpu::BufferUsages::MAP_READ, mapped_at_creation: false });
    let mut encoder = gpu.device().create_command_encoder(&Default::default());
    encoder.copy_texture_to_buffer(
        wgpu::TexelCopyTextureInfo { texture: &other, mip_level: 0, origin: wgpu::Origin3d { x: 100, y: 100, z: 0 }, aspect: wgpu::TextureAspect::All },
        wgpu::TexelCopyBufferInfo { buffer: &out, layout: wgpu::TexelCopyBufferLayout { offset: 0, bytes_per_row: Some(256), rows_per_image: None } },
        wgpu::Extent3d { width: 1, height: 1, depth_or_array_layers: 1 },
    );
    let index = gpu.queue().submit(Some(encoder.finish()));
    out.slice(..).map_async(wgpu::MapMode::Read, |_| {});
    let _ = gpu.device().poll(wgpu::PollType::Wait { submission_index: Some(index), timeout: Some(std::time::Duration::from_secs(2)) });
    let pixel = out.slice(..4).get_mapped_range().map_err(|e| format!("{e:?}"))?.to_vec();
    println!("probe · a pixel of the buffer, read apart (B, G, R, X): {pixel:?} — expected about [255, 127/128, 0, 255]");
    Ok(())
}
