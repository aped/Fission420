//! wgpu resources: pipelines, buffers, and the offscreen color+depth target.
//!
//! The scene is rendered into an offscreen MSAA color + Depth32Float target
//! and resolved into an `Rgba8Unorm` texture that egui displays as a native
//! texture. This keeps depth testing independent of whether eframe's own
//! render pass has a depth attachment.

use eframe::egui;
use eframe::egui_wgpu::{self, wgpu};
use wgpu::util::DeviceExt;

use crate::math::Mat4;

pub const COLOR_FORMAT: wgpu::TextureFormat = wgpu::TextureFormat::Rgba8Unorm;
pub const DEPTH_FORMAT: wgpu::TextureFormat = wgpu::TextureFormat::Depth32Float;

#[repr(C)]
#[derive(Clone, Copy, bytemuck::Pod, bytemuck::Zeroable)]
pub struct MeshVertex {
    pub pos: [f32; 3],
    pub normal: [f32; 3],
    pub group: u32,
}

#[repr(C)]
#[derive(Clone, Copy, bytemuck::Pod, bytemuck::Zeroable)]
pub struct LineVertex {
    pub pos: [f32; 3],
    pub color: [f32; 4],
}

#[repr(C)]
#[derive(Clone, Copy, bytemuck::Pod, bytemuck::Zeroable)]
pub struct Uniforms {
    pub view_proj: Mat4,
    pub eye: [f32; 4],
    pub light_dir: [f32; 4],
    pub base_color: [f32; 4],
    pub highlight_color: [f32; 4],
    pub selection_color: [f32; 4],
    /// Linear RGB to fade the body toward, and the fade amount (0 = off).
    pub fade: [f32; 4],
    /// Section view: unit normal (xyz) and offset (w); see `highlight[2]`.
    pub section: [f32; 4],
    /// x = hovered group, y = 1 if hover highlight active, z = 1 if a section
    /// plane hides material with `n·p > offset`.
    pub highlight: [u32; 4],
}

/// Per-vertex flag bits (second mesh vertex buffer).
pub const FLAG_SELECTED: u32 = 1;
/// Bits 8..11 of the flag word: body tint palette index (0 = base colour).
pub const TINT_SHIFT: u32 = 8;

struct Target {
    size: [u32; 2],
    msaa_view: Option<wgpu::TextureView>,
    depth_view: wgpu::TextureView,
    resolve_view: wgpu::TextureView,
    /// Kept for test read-back.
    #[cfg_attr(not(test), allow(dead_code))]
    resolve_tex: wgpu::Texture,
}

pub struct Gpu {
    render_state: egui_wgpu::RenderState,
    sample_count: u32,
    mesh_pipeline: wgpu::RenderPipeline,
    line_pipeline: wgpu::RenderPipeline,
    uniform_buf: wgpu::Buffer,
    bind_group: wgpu::BindGroup,
    mesh_buf: Option<(wgpu::Buffer, u32)>,
    /// One u32 of `FLAG_*` bits per mesh vertex (same count as `mesh_buf`).
    flag_buf: Option<(wgpu::Buffer, u32)>,
    edge_buf: Option<(wgpu::Buffer, u32)>,
    line_buf: Option<(wgpu::Buffer, u32)>,
    target: Option<Target>,
    texture_id: Option<egui::TextureId>,
}

impl Gpu {
    pub fn new(render_state: &egui_wgpu::RenderState) -> Self {
        let device = &render_state.device;
        let features = render_state
            .adapter
            .get_texture_format_features(COLOR_FORMAT);
        let depth_features = render_state
            .adapter
            .get_texture_format_features(DEPTH_FORMAT);
        let sample_count = if features.flags.sample_count_supported(4)
            && depth_features.flags.sample_count_supported(4)
        {
            4
        } else {
            1
        };

        let shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("f420_viewport_shader"),
            source: wgpu::ShaderSource::Wgsl(include_str!("shader.wgsl").into()),
        });

        let uniform_buf = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("f420_viewport_uniforms"),
            size: std::mem::size_of::<Uniforms>() as u64,
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        let bgl = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("f420_viewport_bgl"),
            entries: &[wgpu::BindGroupLayoutEntry {
                binding: 0,
                visibility: wgpu::ShaderStages::VERTEX_FRAGMENT,
                ty: wgpu::BindingType::Buffer {
                    ty: wgpu::BufferBindingType::Uniform,
                    has_dynamic_offset: false,
                    min_binding_size: None,
                },
                count: None,
            }],
        });
        let bind_group = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("f420_viewport_bg"),
            layout: &bgl,
            entries: &[wgpu::BindGroupEntry {
                binding: 0,
                resource: uniform_buf.as_entire_binding(),
            }],
        });
        let layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("f420_viewport_layout"),
            bind_group_layouts: &[Some(&bgl)],
            immediate_size: 0,
        });

        let depth = |write: bool, bias| wgpu::DepthStencilState {
            format: DEPTH_FORMAT,
            depth_write_enabled: Some(write),
            depth_compare: Some(wgpu::CompareFunction::LessEqual),
            stencil: wgpu::StencilState::default(),
            bias,
        };
        // Polygon offset on the mesh so feature edges lying exactly on faces
        // (and sketch grids on faces) win the depth test. For a float depth
        // buffer the constant term is in units of 2^(e-23) of the primitive's
        // max depth exponent, i.e. it scales with depth precision; the slope
        // term (about one pixel of the face's own depth slope) covers grazing
        // angles. WebGPU forbids depth bias on line topologies, so lines also
        // get a tiny eye-ward nudge in `vs_line`.
        let mesh_bias = wgpu::DepthBiasState {
            constant: 16,
            slope_scale: 1.0,
            clamp: 0.0,
        };
        let multisample = wgpu::MultisampleState {
            count: sample_count,
            mask: !0,
            alpha_to_coverage_enabled: false,
        };

        let mesh_attrs = wgpu::vertex_attr_array![0 => Float32x3, 1 => Float32x3, 2 => Uint32];
        let flag_attrs = wgpu::vertex_attr_array![3 => Uint32];
        let mesh_pipeline = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
            label: Some("f420_mesh_pipeline"),
            layout: Some(&layout),
            vertex: wgpu::VertexState {
                module: &shader,
                entry_point: Some("vs_mesh"),
                compilation_options: Default::default(),
                buffers: &[
                    Some(wgpu::VertexBufferLayout {
                        array_stride: std::mem::size_of::<MeshVertex>() as u64,
                        step_mode: wgpu::VertexStepMode::Vertex,
                        attributes: &mesh_attrs,
                    }),
                    Some(wgpu::VertexBufferLayout {
                        array_stride: std::mem::size_of::<u32>() as u64,
                        step_mode: wgpu::VertexStepMode::Vertex,
                        attributes: &flag_attrs,
                    }),
                ],
            },
            primitive: wgpu::PrimitiveState {
                topology: wgpu::PrimitiveTopology::TriangleList,
                cull_mode: None,
                ..Default::default()
            },
            depth_stencil: Some(depth(true, mesh_bias)),
            multisample,
            fragment: Some(wgpu::FragmentState {
                module: &shader,
                entry_point: Some("fs_mesh"),
                compilation_options: Default::default(),
                targets: &[Some(wgpu::ColorTargetState {
                    format: COLOR_FORMAT,
                    blend: None,
                    write_mask: wgpu::ColorWrites::ALL,
                })],
            }),
            multiview_mask: None,
            cache: None,
        });

        let line_attrs = wgpu::vertex_attr_array![0 => Float32x3, 1 => Float32x4];
        let line_pipeline = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
            label: Some("f420_line_pipeline"),
            layout: Some(&layout),
            vertex: wgpu::VertexState {
                module: &shader,
                entry_point: Some("vs_line"),
                compilation_options: Default::default(),
                buffers: &[Some(wgpu::VertexBufferLayout {
                    array_stride: std::mem::size_of::<LineVertex>() as u64,
                    step_mode: wgpu::VertexStepMode::Vertex,
                    attributes: &line_attrs,
                })],
            },
            primitive: wgpu::PrimitiveState {
                topology: wgpu::PrimitiveTopology::LineList,
                ..Default::default()
            },
            // Lines test against the mesh but do not write depth, so
            // translucent grid lines never occlude each other.
            depth_stencil: Some(depth(false, wgpu::DepthBiasState::default())),
            multisample,
            fragment: Some(wgpu::FragmentState {
                module: &shader,
                entry_point: Some("fs_line"),
                compilation_options: Default::default(),
                targets: &[Some(wgpu::ColorTargetState {
                    format: COLOR_FORMAT,
                    blend: Some(wgpu::BlendState {
                        color: wgpu::BlendComponent {
                            src_factor: wgpu::BlendFactor::SrcAlpha,
                            dst_factor: wgpu::BlendFactor::OneMinusSrcAlpha,
                            operation: wgpu::BlendOperation::Add,
                        },
                        alpha: wgpu::BlendComponent::OVER,
                    }),
                    write_mask: wgpu::ColorWrites::ALL,
                })],
            }),
            multiview_mask: None,
            cache: None,
        });

        Self {
            render_state: render_state.clone(),
            sample_count,
            mesh_pipeline,
            line_pipeline,
            uniform_buf,
            bind_group,
            mesh_buf: None,
            flag_buf: None,
            edge_buf: None,
            line_buf: None,
            target: None,
            texture_id: None,
        }
    }

    /// Upload mesh vertices with their per-vertex flags (`flags.len()` must
    /// equal `verts.len()`, otherwise flags are reset to zero).
    pub fn set_mesh(&mut self, verts: &[MeshVertex], flags: &[u32]) {
        self.mesh_buf = upload(&self.render_state.device, "f420_mesh_vb", verts);
        if flags.len() == verts.len() {
            self.set_mesh_flags(flags);
        } else {
            self.set_mesh_flags(&vec![0; verts.len()]);
        }
    }

    /// Replace only the per-vertex flags (e.g. on selection change).
    pub fn set_mesh_flags(&mut self, flags: &[u32]) {
        self.flag_buf = upload(&self.render_state.device, "f420_mesh_flags", flags);
    }

    pub fn set_edges(&mut self, verts: &[LineVertex]) {
        self.edge_buf = upload(&self.render_state.device, "f420_edge_vb", verts);
    }

    pub fn set_lines(&mut self, verts: &[LineVertex]) {
        self.line_buf = upload(&self.render_state.device, "f420_line_vb", verts);
    }

    fn ensure_target(&mut self, size: [u32; 2]) -> Option<egui::TextureId> {
        let max_dim = self.render_state.device.limits().max_texture_dimension_2d;
        let size = [size[0].clamp(1, max_dim), size[1].clamp(1, max_dim)];
        if self.target.as_ref().is_some_and(|t| t.size == size) {
            return self.texture_id;
        }
        let device = &self.render_state.device;
        let extent = wgpu::Extent3d {
            width: size[0],
            height: size[1],
            depth_or_array_layers: 1,
        };
        let make = |label, format, samples, usage| {
            device.create_texture(&wgpu::TextureDescriptor {
                label: Some(label),
                size: extent,
                mip_level_count: 1,
                sample_count: samples,
                dimension: wgpu::TextureDimension::D2,
                format,
                usage,
                view_formats: &[],
            })
        };
        let view = |t: &wgpu::Texture| t.create_view(&wgpu::TextureViewDescriptor::default());
        let resolve_tex = make(
            "f420_viewport_color",
            COLOR_FORMAT,
            1,
            wgpu::TextureUsages::RENDER_ATTACHMENT
                | wgpu::TextureUsages::TEXTURE_BINDING
                | wgpu::TextureUsages::COPY_SRC,
        );
        let resolve_view = view(&resolve_tex);
        let msaa_view = (self.sample_count > 1).then(|| {
            view(&make(
                "f420_viewport_msaa",
                COLOR_FORMAT,
                self.sample_count,
                wgpu::TextureUsages::RENDER_ATTACHMENT,
            ))
        });
        let depth_view = view(&make(
            "f420_viewport_depth",
            DEPTH_FORMAT,
            self.sample_count,
            wgpu::TextureUsages::RENDER_ATTACHMENT,
        ));

        let mut renderer = self.render_state.renderer.write();
        let id = match self.texture_id {
            Some(id) => {
                renderer.update_egui_texture_from_wgpu_texture(
                    device,
                    &resolve_view,
                    wgpu::FilterMode::Nearest,
                    id,
                );
                id
            }
            None => {
                renderer.register_native_texture(device, &resolve_view, wgpu::FilterMode::Nearest)
            }
        };
        drop(renderer);
        self.texture_id = Some(id);
        self.target = Some(Target {
            size,
            msaa_view,
            depth_view,
            resolve_view,
            resolve_tex,
        });
        Some(id)
    }

    /// Render the scene at `size` physical pixels; returns the egui texture.
    pub fn render(
        &mut self,
        size: [u32; 2],
        uniforms: &Uniforms,
        clear: [f64; 3],
    ) -> Option<egui::TextureId> {
        let id = self.ensure_target(size)?;
        let target = self.target.as_ref()?;
        let queue = &self.render_state.queue;
        queue.write_buffer(&self.uniform_buf, 0, bytemuck::bytes_of(uniforms));

        let mut encoder =
            self.render_state
                .device
                .create_command_encoder(&wgpu::CommandEncoderDescriptor {
                    label: Some("f420_viewport_encoder"),
                });
        {
            let (view, resolve_target) = match &target.msaa_view {
                Some(msaa) => (msaa, Some(&target.resolve_view)),
                None => (&target.resolve_view, None),
            };
            let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: Some("f420_viewport_pass"),
                color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                    view,
                    depth_slice: None,
                    resolve_target,
                    ops: wgpu::Operations {
                        load: wgpu::LoadOp::Clear(wgpu::Color {
                            r: clear[0],
                            g: clear[1],
                            b: clear[2],
                            a: 1.0,
                        }),
                        store: wgpu::StoreOp::Store,
                    },
                })],
                depth_stencil_attachment: Some(wgpu::RenderPassDepthStencilAttachment {
                    view: &target.depth_view,
                    depth_ops: Some(wgpu::Operations {
                        load: wgpu::LoadOp::Clear(1.0),
                        store: wgpu::StoreOp::Discard,
                    }),
                    stencil_ops: None,
                }),
                timestamp_writes: None,
                occlusion_query_set: None,
                multiview_mask: None,
            });
            pass.set_bind_group(0, &self.bind_group, &[]);
            if let (Some((buf, n)), Some((flags, nf))) = (&self.mesh_buf, &self.flag_buf)
                && n == nf
            {
                pass.set_pipeline(&self.mesh_pipeline);
                pass.set_vertex_buffer(0, buf.slice(..));
                pass.set_vertex_buffer(1, flags.slice(..));
                pass.draw(0..*n, 0..1);
            }
            pass.set_pipeline(&self.line_pipeline);
            for (buf, n) in [&self.edge_buf, &self.line_buf].into_iter().flatten() {
                pass.set_vertex_buffer(0, buf.slice(..));
                pass.draw(0..*n, 0..1);
            }
        }
        // Submitted before eframe submits this frame's egui pass, which
        // samples the resolved texture.
        queue.submit(Some(encoder.finish()));
        Some(id)
    }
}

#[cfg(test)]
impl Gpu {
    /// Copy the last rendered frame back to the CPU as tightly packed RGBA8.
    pub fn read_back(&self) -> Option<(Vec<u8>, [u32; 2])> {
        let t = self.target.as_ref()?;
        let [w, h] = t.size;
        let device = &self.render_state.device;
        let row = (w * 4).div_ceil(256) * 256;
        let buf = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("f420_readback"),
            size: u64::from(row) * u64::from(h),
            usage: wgpu::BufferUsages::COPY_DST | wgpu::BufferUsages::MAP_READ,
            mapped_at_creation: false,
        });
        let mut enc = device.create_command_encoder(&Default::default());
        enc.copy_texture_to_buffer(
            wgpu::TexelCopyTextureInfo {
                texture: &t.resolve_tex,
                mip_level: 0,
                origin: wgpu::Origin3d::ZERO,
                aspect: wgpu::TextureAspect::All,
            },
            wgpu::TexelCopyBufferInfo {
                buffer: &buf,
                layout: wgpu::TexelCopyBufferLayout {
                    offset: 0,
                    bytes_per_row: Some(row),
                    rows_per_image: Some(h),
                },
            },
            wgpu::Extent3d {
                width: w,
                height: h,
                depth_or_array_layers: 1,
            },
        );
        self.render_state.queue.submit(Some(enc.finish()));
        buf.slice(..).map_async(wgpu::MapMode::Read, |_| {});
        device.poll(wgpu::PollType::wait_indefinitely()).ok()?;
        let data = buf.slice(..).get_mapped_range().ok()?.to_vec();
        let mut out = Vec::with_capacity((w * h * 4) as usize);
        for y in 0..h as usize {
            let start = y * row as usize;
            out.extend_from_slice(data.get(start..start + (w * 4) as usize)?);
        }
        Some((out, [w, h]))
    }
}

impl Drop for Gpu {
    fn drop(&mut self) {
        if let Some(id) = self.texture_id.take() {
            self.render_state.renderer.write().free_texture(&id);
        }
    }
}

fn upload<T: bytemuck::Pod>(
    device: &wgpu::Device,
    label: &str,
    data: &[T],
) -> Option<(wgpu::Buffer, u32)> {
    let count = u32::try_from(data.len()).ok()?;
    if count == 0 {
        return None;
    }
    let buf = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
        label: Some(label),
        contents: bytemuck::cast_slice(data),
        usage: wgpu::BufferUsages::VERTEX,
    });
    Some((buf, count))
}
