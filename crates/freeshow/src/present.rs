// SPDX-License-Identifier: GPL-3.0-only

use std::cell::RefCell;
use std::rc::Rc;
use std::sync::atomic::{AtomicBool, Ordering};

use freeshow_output::Frame;
use slint::wgpu_30::wgpu;

static SOFTWARE: AtomicBool = AtomicBool::new(false);

/// Picks the renderer: femtovg on wgpu when a GPU adapter exists, otherwise
/// Slint's software renderer so the app still opens on VMs, remote desktops and
/// machines without working GPU drivers. `FREESHOW_RENDERER=software` forces it.
pub fn select_renderer() -> Result<(), slint::PlatformError> {
    let forced = std::env::var("FREESHOW_RENDERER").is_ok_and(|value| value.trim() == "software");
    if !forced {
        match slint::BackendSelector::new()
            .backend_name("winit".into())
            .renderer_name("femtovg-wgpu".into())
            .require_wgpu_30(slint::wgpu_30::WGPUConfiguration::default())
            .select()
        {
            Ok(()) => return Ok(()),
            Err(err) => tracing::warn!("no GPU renderer ({err}); using the software renderer"),
        }
    }
    slint::BackendSelector::new()
        .backend_name("winit".into())
        .renderer_name("software".into())
        .select()?;
    SOFTWARE.store(true, Ordering::Relaxed);
    Ok(())
}

/// True when frames are copied into images instead of uploaded with wgpu.
pub fn software_rendering() -> bool {
    SOFTWARE.load(Ordering::Relaxed)
}

type ApplyFrame = Rc<dyn Fn(slint::Image)>;

/// A frame and the window property that should receive it.
pub type FrameTarget = (Frame, Box<dyn Fn(slint::Image)>);

struct PendingFrame {
    frame: Frame,
    apply: ApplyFrame,
}

/// Holds the next compositor frame and uploads it with wgpu on the next render.
/// Slide pixels are produced by `freeshow-output`, not by Slint widgets.
/// Several slots can share one window and its single rendering notifier.
pub struct FrameSlot {
    pending: Rc<RefCell<Vec<PendingFrame>>>,
    apply: ApplyFrame,
    redraw: Rc<dyn Fn()>,
    /// Software renderer: frames become images right away.
    direct: Option<ApplyFrame>,
}

impl FrameSlot {
    pub fn publish(&self, frame: Frame) {
        if let Some(apply) = &self.direct {
            apply(frame_image(&frame));
            return;
        }
        self.pending.borrow_mut().push(PendingFrame {
            frame,
            apply: Rc::clone(&self.apply),
        });
        (self.redraw)();
    }
}

pub fn frame_image(frame: &Frame) -> slint::Image {
    let buffer = slint::SharedPixelBuffer::<slint::Rgba8Pixel>::clone_from_slice(
        &frame.pixels,
        frame.width,
        frame.height,
    );
    slint::Image::from_rgba8(buffer)
}

pub fn attach_frame(
    window: &slint::Window,
    initial: Frame,
    apply: impl Fn(slint::Image) + 'static,
    redraw: impl Fn() + 'static,
) -> Result<FrameSlot, slint::PlatformError> {
    let mut slots = attach_frames(window, vec![(initial, Box::new(apply))], redraw)?;
    Ok(slots.remove(0))
}

/// Attaches every frame slot of one window. Slint allows a single rendering
/// notifier per window, so slots that share a window must share the notifier.
pub fn attach_frames(
    window: &slint::Window,
    frames: Vec<FrameTarget>,
    redraw: impl Fn() + 'static,
) -> Result<Vec<FrameSlot>, slint::PlatformError> {
    let redraw: Rc<dyn Fn()> = Rc::new(redraw);
    if software_rendering() {
        return Ok(frames
            .into_iter()
            .map(|(initial, apply)| {
                let apply: ApplyFrame = Rc::from(apply);
                apply(frame_image(&initial));
                FrameSlot {
                    pending: Rc::new(RefCell::new(Vec::new())),
                    apply: Rc::clone(&apply),
                    redraw: Rc::clone(&redraw),
                    direct: Some(apply),
                }
            })
            .collect());
    }
    let pending = Rc::new(RefCell::new(Vec::new()));
    let mut applies = Vec::with_capacity(frames.len());
    for (frame, apply) in frames {
        let apply: ApplyFrame = Rc::from(apply);
        pending.borrow_mut().push(PendingFrame {
            frame,
            apply: Rc::clone(&apply),
        });
        applies.push(apply);
    }
    let pending_for_render = Rc::clone(&pending);
    window
        .set_rendering_notifier(move |state, api| {
            if !matches!(state, slint::RenderingState::BeforeRendering) {
                return;
            }
            let frames = std::mem::take(&mut *pending_for_render.borrow_mut());
            if frames.is_empty() {
                return;
            }
            let slint::GraphicsAPI::WGPU30 { device, queue, .. } = api else {
                tracing::error!("output window is not using the wgpu renderer");
                return;
            };
            for PendingFrame { frame, apply } in frames {
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
            }
        })
        .map_err(|err| slint::PlatformError::from(err.to_string()))?;
    Ok(applies
        .into_iter()
        .map(|apply| FrameSlot {
            pending: Rc::clone(&pending),
            apply,
            redraw: Rc::clone(&redraw),
            direct: None,
        })
        .collect())
}
