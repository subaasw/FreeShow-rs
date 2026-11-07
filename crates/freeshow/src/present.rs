// SPDX-License-Identifier: GPL-3.0-only

use std::cell::RefCell;
use std::rc::Rc;

use freeshow_output::Frame;
use slint::wgpu_30::wgpu;

/// Holds the next compositor frame and uploads it with wgpu on the next render.
/// Slide pixels are produced by `freeshow-output`, not by Slint widgets.
pub struct FrameSlot {
    pending: Rc<RefCell<Option<Frame>>>,
    redraw: Rc<dyn Fn()>,
}

impl FrameSlot {
    pub fn publish(&self, frame: Frame) {
        *self.pending.borrow_mut() = Some(frame);
        (self.redraw)();
    }
}

pub fn attach_frame(
    window: &slint::Window,
    initial: Frame,
    apply: impl Fn(slint::Image) + 'static,
    redraw: impl Fn() + 'static,
) -> Result<FrameSlot, slint::PlatformError> {
    let pending = Rc::new(RefCell::new(Some(initial)));
    let pending_for_render = Rc::clone(&pending);
    window
        .set_rendering_notifier(move |state, api| {
            if !matches!(state, slint::RenderingState::BeforeRendering) {
                return;
            }
            let Some(frame) = pending_for_render.borrow_mut().take() else {
                return;
            };
            let slint::GraphicsAPI::WGPU30 { device, queue, .. } = api else {
                tracing::error!("output window is not using the wgpu renderer");
                return;
            };
            let width = frame.width;
            let height = frame.height;
            let size = wgpu::Extent3d {
                width,
                height,
                depth_or_array_layers: 1,
            };
            let texture = device.create_texture(&wgpu::TextureDescriptor {
                label: Some("freeshow-frame"),
                size,
                mip_level_count: 1,
                sample_count: 1,
                dimension: wgpu::TextureDimension::D2,
                format: wgpu::TextureFormat::Rgba8Unorm,
                usage: wgpu::TextureUsages::TEXTURE_BINDING
                    | wgpu::TextureUsages::COPY_DST
                    | wgpu::TextureUsages::RENDER_ATTACHMENT,
                view_formats: &[],
            });
            queue.write_texture(
                wgpu::TexelCopyTextureInfo {
                    texture: &texture,
                    mip_level: 0,
                    origin: wgpu::Origin3d::ZERO,
                    aspect: wgpu::TextureAspect::All,
                },
                &frame.pixels,
                wgpu::TexelCopyBufferLayout {
                    offset: 0,
                    bytes_per_row: Some(width * 4),
                    rows_per_image: Some(height),
                },
                size,
            );
            match slint::Image::try_from(texture) {
                Ok(image) => apply(image),
                Err(err) => tracing::error!("could not import output frame: {err}"),
            }
        })
        .map_err(|err| slint::PlatformError::from(err.to_string()))?;
    Ok(FrameSlot {
        pending,
        redraw: Rc::new(redraw),
    })
}
