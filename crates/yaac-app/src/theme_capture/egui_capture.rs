//! Feed the standard demo view through the egui renderer into a texture without a window.
use std::path::Path;

use eframe::{egui, egui_wgpu, wgpu};
use yaac_core::demo::Scenario;

use super::{HEIGHT, ThemeInput, WIDTH, display_error};
use crate::{Launch, app::YaacApp};

pub(super) async fn render(theme: ThemeInput, path: &Path) -> Result<(), String> {
    let ctx = egui::Context::default();
    let mut app = YaacApp::from_ctx(
        &ctx,
        Launch {
            demo: Some(Scenario::Fleet),
            theme: crate::theme::ThemeChoice::Dark,
            shots: None,
            host_windows: cfg!(windows),
            #[cfg(feature = "hub")]
            attach: None,
            display: Default::default(),
            shell: crate::tray::Shell::new(),
            tray: false,
            updater: crate::updater::Updater::off("Offline demo"),
        },
    );
    theme.apply(&mut app);
    let screen_rect =
        egui::Rect::from_min_size(egui::Pos2::ZERO, egui::vec2(WIDTH as f32, HEIGHT as f32));
    let mut frames = egui::FullOutput::default();
    for i in 0..8 {
        let raw = egui::RawInput {
            screen_rect: Some(screen_rect),
            time: Some(i as f64 * 0.12),
            ..Default::default()
        };
        frames.append(ctx.run_ui(raw, |ui| {
            app.upkeep(ui.ctx());
            app.paint_window(ui);
        }));
    }

    let mut descriptor = wgpu::InstanceDescriptor::new_without_display_handle();
    descriptor.backends = if cfg!(windows) {
        wgpu::Backends::DX12
    } else {
        wgpu::Backends::VULKAN | wgpu::Backends::GL
    };
    let instance = wgpu::Instance::new(descriptor);
    let mut options = wgpu::RequestAdapterOptions {
        power_preference: wgpu::PowerPreference::LowPower,
        force_fallback_adapter: true,
        compatible_surface: None,
        ..Default::default()
    };
    let adapter = match instance.request_adapter(&options).await {
        Ok(adapter) => adapter,
        Err(_) => {
            options.force_fallback_adapter = false;
            instance
                .request_adapter(&options)
                .await
                .map_err(display_error)?
        }
    };
    let (device, queue) = adapter
        .request_device(&wgpu::DeviceDescriptor::default())
        .await
        .map_err(display_error)?;
    let format = wgpu::TextureFormat::Rgba8Unorm;
    let size = wgpu::Extent3d {
        width: WIDTH,
        height: HEIGHT,
        depth_or_array_layers: 1,
    };
    let image = device.create_texture(&wgpu::TextureDescriptor {
        label: Some("YAAC demo screenshot"),
        size,
        mip_level_count: 1,
        sample_count: 1,
        dimension: wgpu::TextureDimension::D2,
        format,
        usage: wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::COPY_SRC,
        view_formats: &[],
    });
    let mut renderer = egui_wgpu::Renderer::new(&device, format, Default::default());
    for (id, deltas) in &frames.textures_delta.set {
        for delta in deltas {
            renderer.update_texture(&device, &queue, *id, delta);
        }
    }
    frames.textures_delta.clear();
    let pixels_per_point = frames.pixels_per_point;
    let paint = ctx.tessellate(frames.shapes, pixels_per_point);
    let screen = egui_wgpu::ScreenDescriptor {
        size_in_pixels: [WIDTH, HEIGHT],
        pixels_per_point,
    };
    let mut encoder = device.create_command_encoder(&Default::default());
    let jobs = renderer.update_buffers(&device, &queue, &mut encoder, &paint, &screen);
    let view = image.create_view(&Default::default());
    {
        let attachments = [Some(wgpu::RenderPassColorAttachment {
            view: &view,
            depth_slice: None,
            resolve_target: None,
            ops: wgpu::Operations {
                load: wgpu::LoadOp::Clear(wgpu::Color::BLACK),
                store: wgpu::StoreOp::Store,
            },
        })];
        let pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
            color_attachments: &attachments,
            ..Default::default()
        });
        renderer.render(&mut pass.forget_lifetime(), &paint, &screen);
    }

    let row = WIDTH * 4;
    let buffer = device.create_buffer(&wgpu::BufferDescriptor {
        label: Some("YAAC screenshot readback"),
        size: u64::from(row * HEIGHT),
        usage: wgpu::BufferUsages::COPY_DST | wgpu::BufferUsages::MAP_READ,
        mapped_at_creation: false,
    });
    encoder.copy_texture_to_buffer(
        image.as_image_copy(),
        wgpu::TexelCopyBufferInfo {
            buffer: &buffer,
            layout: wgpu::TexelCopyBufferLayout {
                offset: 0,
                bytes_per_row: Some(row),
                rows_per_image: Some(HEIGHT),
            },
        },
        size,
    );
    let submission = queue.submit(jobs.into_iter().chain(std::iter::once(encoder.finish())));
    let (sender, receiver) = std::sync::mpsc::channel();
    buffer
        .slice(..)
        .map_async(wgpu::MapMode::Read, move |result| {
            let _ = sender.send(result);
        });
    device
        .poll(wgpu::PollType::Wait {
            submission_index: Some(submission),
            timeout: Some(std::time::Duration::from_secs(5)),
        })
        .map_err(display_error)?;
    receiver
        .recv_timeout(std::time::Duration::from_secs(1))
        .map_err(display_error)?
        .map_err(display_error)?;
    let pixels = buffer.slice(..).get_mapped_range().map_err(display_error)?;
    image::save_buffer_with_format(
        path,
        &pixels,
        WIDTH,
        HEIGHT,
        image::ColorType::Rgba8,
        image::ImageFormat::Png,
    )
    .map_err(display_error)
}
