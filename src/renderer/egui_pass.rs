use anyhow::Result;
use egui_wgpu::{Renderer as EguiRenderer, ScreenDescriptor};

use super::{GpuTimer, SurfaceFrame};

pub fn render(
    timer: &mut GpuTimer,
    device: &wgpu::Device,
    queue: &wgpu::Queue,
    painter: &mut EguiRenderer,
    paint_jobs: &[egui::ClippedPrimitive],
    screen: &ScreenDescriptor,
    frame: SurfaceFrame,
) -> Result<()> {
    let view = frame.view();
    let mut encoder =
        device.create_command_encoder(&wgpu::CommandEncoderDescriptor { label: Some("Egui Encoder") });
    timer.write_timestamp(&mut encoder, super::GpuTimestampLabel::EguiStart);
    let mut extra_cmd = painter.update_buffers(device, queue, &mut encoder, paint_jobs, screen);

    {
        let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
            label: Some("Egui Pass"),
            color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                view,
                depth_slice: None,
                resolve_target: None,
                ops: wgpu::Operations { load: wgpu::LoadOp::Load, store: wgpu::StoreOp::Store },
            })],
            depth_stencil_attachment: None,
            occlusion_query_set: None,
            timestamp_writes: None,
        });
        let pass = unsafe {
            std::mem::transmute::<&mut wgpu::RenderPass<'_>, &mut wgpu::RenderPass<'static>>(&mut pass)
        };
        painter.render(pass, paint_jobs, screen);
    }
    timer.write_timestamp(&mut encoder, super::GpuTimestampLabel::EguiEnd);
    timer.finish_frame(&mut encoder);
    extra_cmd.push(encoder.finish());
    queue.submit(extra_cmd.into_iter());
    timer.collect_results(device);
    frame.present();
    Ok(())
}
