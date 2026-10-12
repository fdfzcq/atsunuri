use std::ffi::c_void;
use std::ptr::NonNull;

use atsunuri_core::{Canvas, Rect};
use raw_window_handle::{
    DisplayHandle, HandleError, HasDisplayHandle, RawDisplayHandle, RawWindowHandle,
    WaylandDisplayHandle, WaylandWindowHandle,
};

use crate::view::View;

/// Mid grey drawn around the canvas, sRGB-encoded.
const BACKGROUND_SRGB: [f32; 3] = [0.5, 0.5, 0.5];

/// Draws the canvas into a Wayland surface with wgpu.
pub struct Renderer {
    // Declared first so it is dropped before the device it was created from.
    surface: wgpu::Surface<'static>,
    device: wgpu::Device,
    queue: wgpu::Queue,
    config: wgpu::SurfaceConfiguration,
    pipeline: wgpu::RenderPipeline,
    bind_group: wgpu::BindGroup,
    uniforms: wgpu::Buffer,
    canvas_texture: wgpu::Texture,
    canvas_size: (u32, u32),
}

impl Renderer {
    /// # Safety
    ///
    /// `display` and `surface` must be live `wl_display` and `wl_surface`
    /// pointers that outlive the returned renderer.
    pub unsafe fn new(
        display: NonNull<c_void>,
        surface: NonNull<c_void>,
        canvas_size: (u32, u32),
    ) -> Result<Self, String> {
        let instance = wgpu::Instance::new(wgpu::InstanceDescriptor {
            backends: wgpu::Backends::VULKAN | wgpu::Backends::GL,
            // The GL backend needs the display up front on Wayland.
            display: Some(Box::new(WaylandDisplay(display))),
            ..wgpu::InstanceDescriptor::new_without_display_handle()
        });

        let target = wgpu::SurfaceTargetUnsafe::RawHandle {
            raw_display_handle: Some(RawDisplayHandle::Wayland(WaylandDisplayHandle::new(
                display,
            ))),
            raw_window_handle: RawWindowHandle::Wayland(WaylandWindowHandle::new(surface)),
        };
        let surface = unsafe { instance.create_surface_unsafe(target) }
            .map_err(|e| format!("failed to create surface: {e}"))?;

        let adapter = pollster::block_on(instance.request_adapter(&wgpu::RequestAdapterOptions {
            compatible_surface: Some(&surface),
            ..Default::default()
        }))
        .map_err(|e| format!("no suitable GPU adapter: {e}"))?;
        log::info!("using {:?}", adapter.get_info());

        let (device, queue) = pollster::block_on(adapter.request_device(&wgpu::DeviceDescriptor {
            label: Some("atsunuri"),
            required_limits:
                wgpu::Limits::downlevel_webgl2_defaults().using_resolution(adapter.limits()),
            ..Default::default()
        }))
        .map_err(|e| format!("failed to open GPU device: {e}"))?;

        let mut config = surface
            .get_default_config(&adapter, 1, 1)
            .ok_or("surface is not supported by the adapter")?;
        let caps = surface.get_capabilities(&adapter);
        // Prefer an sRGB surface. Either way the canvas texture uses the same
        // encoding as the surface, so its sRGB values pass through unchanged.
        config.format = caps
            .formats
            .iter()
            .copied()
            .find(|f| f.is_srgb())
            .unwrap_or(caps.formats[0]);
        config.view_formats = vec![config.format];
        if caps.present_modes.contains(&wgpu::PresentMode::Mailbox) {
            // Frame pacing comes from Wayland frame callbacks instead.
            config.present_mode = wgpu::PresentMode::Mailbox;
        }
        if caps.alpha_modes.contains(&wgpu::CompositeAlphaMode::Opaque) {
            config.alpha_mode = wgpu::CompositeAlphaMode::Opaque;
        }
        config.desired_maximum_frame_latency = 1;

        let canvas_format = if config.format.is_srgb() {
            wgpu::TextureFormat::Rgba8UnormSrgb
        } else {
            wgpu::TextureFormat::Rgba8Unorm
        };
        let canvas_texture = device.create_texture(&wgpu::TextureDescriptor {
            label: Some("canvas"),
            size: wgpu::Extent3d {
                width: canvas_size.0,
                height: canvas_size.1,
                depth_or_array_layers: 1,
            },
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: canvas_format,
            usage: wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_DST,
            view_formats: &[],
        });
        let sampler = device.create_sampler(&wgpu::SamplerDescriptor {
            label: Some("canvas"),
            mag_filter: wgpu::FilterMode::Nearest,
            min_filter: wgpu::FilterMode::Linear,
            ..Default::default()
        });
        let uniforms = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("view"),
            size: std::mem::size_of::<[f32; 12]>() as u64,
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });

        let layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("canvas"),
            entries: &[
                wgpu::BindGroupLayoutEntry {
                    binding: 0,
                    visibility: wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Buffer {
                        ty: wgpu::BufferBindingType::Uniform,
                        has_dynamic_offset: false,
                        min_binding_size: None,
                    },
                    count: None,
                },
                wgpu::BindGroupLayoutEntry {
                    binding: 1,
                    visibility: wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Texture {
                        sample_type: wgpu::TextureSampleType::Float { filterable: true },
                        view_dimension: wgpu::TextureViewDimension::D2,
                        multisampled: false,
                    },
                    count: None,
                },
                wgpu::BindGroupLayoutEntry {
                    binding: 2,
                    visibility: wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Sampler(wgpu::SamplerBindingType::Filtering),
                    count: None,
                },
            ],
        });
        let bind_group = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("canvas"),
            layout: &layout,
            entries: &[
                wgpu::BindGroupEntry {
                    binding: 0,
                    resource: uniforms.as_entire_binding(),
                },
                wgpu::BindGroupEntry {
                    binding: 1,
                    resource: wgpu::BindingResource::TextureView(
                        &canvas_texture.create_view(&Default::default()),
                    ),
                },
                wgpu::BindGroupEntry {
                    binding: 2,
                    resource: wgpu::BindingResource::Sampler(&sampler),
                },
            ],
        });

        let shader = device.create_shader_module(wgpu::include_wgsl!("canvas.wgsl"));
        let pipeline_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("canvas"),
            bind_group_layouts: &[Some(&layout)],
            immediate_size: 0,
        });
        let pipeline = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
            label: Some("canvas"),
            layout: Some(&pipeline_layout),
            vertex: wgpu::VertexState {
                module: &shader,
                entry_point: Some("vs_main"),
                compilation_options: Default::default(),
                buffers: &[],
            },
            primitive: Default::default(),
            depth_stencil: None,
            multisample: Default::default(),
            fragment: Some(wgpu::FragmentState {
                module: &shader,
                entry_point: Some("fs_main"),
                compilation_options: Default::default(),
                targets: &[Some(config.format.into())],
            }),
            multiview_mask: None,
            cache: None,
        });

        Ok(Renderer {
            surface,
            device,
            queue,
            config,
            pipeline,
            bind_group,
            uniforms,
            canvas_texture,
            canvas_size,
        })
    }

    /// Resizes the swapchain, in physical pixels.
    pub fn resize(&mut self, width: u32, height: u32) {
        self.config.width = width.max(1);
        self.config.height = height.max(1);
        self.surface.configure(&self.device, &self.config);
    }

    pub fn size(&self) -> (u32, u32) {
        (self.config.width, self.config.height)
    }

    /// Copies the `dirty` region of `canvas` to the GPU.
    pub fn upload(&self, canvas: &Canvas, dirty: Rect) {
        debug_assert_eq!((canvas.width(), canvas.height()), self.canvas_size);
        let row = canvas.width() * 4;
        self.queue.write_texture(
            wgpu::TexelCopyTextureInfo {
                texture: &self.canvas_texture,
                mip_level: 0,
                origin: wgpu::Origin3d {
                    x: dirty.x0,
                    y: dirty.y0,
                    z: 0,
                },
                aspect: wgpu::TextureAspect::All,
            },
            canvas.pixels(),
            wgpu::TexelCopyBufferLayout {
                offset: (dirty.y0 * row + dirty.x0 * 4) as u64,
                bytes_per_row: Some(row),
                rows_per_image: Some(dirty.height()),
            },
            wgpu::Extent3d {
                width: dirty.width(),
                height: dirty.height(),
                depth_or_array_layers: 1,
            },
        );
    }

    /// Draws one frame. Returns false if the frame was skipped.
    pub fn render(&mut self, view: &View) -> bool {
        let frame = match self.surface.get_current_texture() {
            wgpu::CurrentSurfaceTexture::Success(frame) => frame,
            wgpu::CurrentSurfaceTexture::Suboptimal(frame) => {
                drop(frame);
                self.surface.configure(&self.device, &self.config);
                return false;
            }
            wgpu::CurrentSurfaceTexture::Outdated | wgpu::CurrentSurfaceTexture::Lost => {
                self.surface.configure(&self.device, &self.config);
                return false;
            }
            other => {
                log::warn!("skipping frame: {other:?}");
                return false;
            }
        };

        let background = if self.config.format.is_srgb() {
            BACKGROUND_SRGB.map(srgb_to_linear)
        } else {
            BACKGROUND_SRGB
        };
        let uniforms: [f32; 12] = [
            view.offset[0],
            view.offset[1],
            view.scale,
            0.0,
            self.canvas_size.0 as f32,
            self.canvas_size.1 as f32,
            0.0,
            0.0,
            background[0],
            background[1],
            background[2],
            1.0,
        ];
        let bytes: Vec<u8> = uniforms.iter().flat_map(|v| v.to_ne_bytes()).collect();
        self.queue.write_buffer(&self.uniforms, 0, &bytes);

        let target = frame.texture.create_view(&Default::default());
        let mut encoder = self.device.create_command_encoder(&Default::default());
        {
            let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: Some("canvas"),
                color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                    view: &target,
                    depth_slice: None,
                    resolve_target: None,
                    ops: wgpu::Operations {
                        load: wgpu::LoadOp::Clear(wgpu::Color::BLACK),
                        store: wgpu::StoreOp::Store,
                    },
                })],
                depth_stencil_attachment: None,
                timestamp_writes: None,
                occlusion_query_set: None,
                multiview_mask: None,
            });
            pass.set_pipeline(&self.pipeline);
            pass.set_bind_group(0, &self.bind_group, &[]);
            pass.draw(0..3, 0..1);
        }
        self.queue.submit([encoder.finish()]);
        self.queue.present(frame);
        true
    }
}

fn srgb_to_linear(c: f32) -> f32 {
    if c <= 0.04045 {
        c / 12.92
    } else {
        ((c + 0.055) / 1.055).powf(2.4)
    }
}

/// The Wayland display, handed to wgpu so its GL backend can use it.
#[derive(Debug)]
struct WaylandDisplay(NonNull<c_void>);

// SAFETY: a wl_display pointer may be used from any thread; libwayland
// synchronizes access to it internally.
unsafe impl Send for WaylandDisplay {}
unsafe impl Sync for WaylandDisplay {}

impl HasDisplayHandle for WaylandDisplay {
    fn display_handle(&self) -> Result<DisplayHandle<'_>, HandleError> {
        let raw = RawDisplayHandle::Wayland(WaylandDisplayHandle::new(self.0));
        // SAFETY: the caller of `Renderer::new` guarantees the display outlives the renderer.
        Ok(unsafe { DisplayHandle::borrow_raw(raw) })
    }
}
