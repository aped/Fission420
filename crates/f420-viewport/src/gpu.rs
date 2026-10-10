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
    pub highlight: [u32; 4],
}

struct Target {
    size: [u32; 2],
    msaa_view: Option<wgpu::TextureView>,
    depth_view: wgpu::TextureView,
    resolve_view: wgpu::TextureView,
}

pub struct Gpu {
    render_state: egui_wgpu::RenderState,
    sample_count: u32,
    mesh_pipeline: wgpu::RenderPipeline,
    line_pipeline: wgpu::RenderPipeline,
    uniform_buf: wgpu::Buffer,
    bind_group: wgpu::BindGroup,
    mesh_buf: Option<(wgpu::Buffer, u32)>,
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

        let depth = |write: bool| wgpu::DepthStencilState {
            format: DEPTH_FORMAT,
            depth_write_enabled: Some(write),
            depth_compare: Some(wgpu::CompareFunction::LessEqual),
            stencil: wgpu::StencilState::default(),
            bias: wgpu::DepthBiasState::default(),
        };
        let multisample = wgpu::MultisampleState {
            count: sample_count,
            mask: !0,
            alpha_to_coverage_enabled: false,
        };

        let mesh_attrs = wgpu::vertex_attr_array![0 => Float32x3, 1 => Float32x3, 2 => Uint32];
        let mesh_pipeline = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
            label: Some("f420_mesh_pipeline"),
            layout: Some(&layout),
            vertex: wgpu::VertexState {
                module: &shader,
                entry_point: Some("vs_mesh"),
                compilation_options: Default::default(),
                buffers: &[Some(wgpu::VertexBufferLayout {
                    array_stride: std::mem::size_of::<MeshVertex>() as u64,
                    step_mode: wgpu::VertexStepMode::Vertex,
                    attributes: &mesh_attrs,
                })],
            },
            primitive: wgpu::PrimitiveState {
                topology: wgpu::PrimitiveTopology::TriangleList,
                cull_mode: None,
                ..Default::default()
            },
            depth_stencil: Some(depth(true)),
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
            depth_stencil: Some(depth(false)),
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
            line_buf: None,
            target: None,
            texture_id: None,
        }
    }

    pub fn set_mesh(&mut self, verts: &[MeshVertex]) {
        self.mesh_buf = upload(&self.render_state.device, "f420_mesh_vb", verts);
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
            device
                .create_texture(&wgpu::TextureDescriptor {
                    label: Some(label),
                    size: extent,
                    mip_level_count: 1,
                    sample_count: samples,
                    dimension: wgpu::TextureDimension::D2,
                    format,
                    usage,
                    view_formats: &[],
                })
                .create_view(&wgpu::TextureViewDescriptor::default())
        };
        let resolve_view = make(
            "f420_viewport_color",
            COLOR_FORMAT,
            1,
            wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::TEXTURE_BINDING,
        );
        let msaa_view = (self.sample_count > 1).then(|| {
            make(
                "f420_viewport_msaa",
                COLOR_FORMAT,
                self.sample_count,
                wgpu::TextureUsages::RENDER_ATTACHMENT,
            )
        });
        let depth_view = make(
            "f420_viewport_depth",
            DEPTH_FORMAT,
            self.sample_count,
            wgpu::TextureUsages::RENDER_ATTACHMENT,
        );

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
            if let Some((buf, n)) = &self.mesh_buf {
                pass.set_pipeline(&self.mesh_pipeline);
                pass.set_vertex_buffer(0, buf.slice(..));
                pass.draw(0..*n, 0..1);
            }
            if let Some((buf, n)) = &self.line_buf {
                pass.set_pipeline(&self.line_pipeline);
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
