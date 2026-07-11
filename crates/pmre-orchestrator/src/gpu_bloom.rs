//! GPU-accelerated additive Gaussian bloom using wgpu compute shaders.
//! Three compute dispatches: bright-pass → horizontal Gaussian → vertical Gaussian + composite.
//! Falls back to the CPU path transparently when wgpu initialisation fails.

use pmre_kit::{framebuffer::Framebuffer, paint::Rgba, post};
use wgpu::util::DeviceExt;

// ── Embedded WGSL shaders ─────────────────────────────────────────────────────

const BRIGHT_WGSL: &str = r#"
struct Params { width: u32, height: u32, threshold: f32, _pad: u32 }

@group(0) @binding(0) var<storage, read>        src:    array<f32>;
@group(0) @binding(1) var<storage, read_write>  bright: array<f32>;
@group(0) @binding(2) var<uniform>              params: Params;

fn luma(r: f32, g: f32, b: f32) -> f32 {
    return 0.299 * r + 0.587 * g + 0.114 * b;
}

@compute @workgroup_size(64)
fn main(@builtin(global_invocation_id) id: vec3<u32>) {
    let i = id.x;
    if i >= params.width * params.height { return; }
    let base = i * 4u;
    let r = src[base]; let g = src[base + 1u]; let bv = src[base + 2u];
    let l = luma(r, g, bv);
    if l > params.threshold {
        let s = (l - params.threshold) / l;
        bright[base]      = r  * s;
        bright[base + 1u] = g  * s;
        bright[base + 2u] = bv * s;
        bright[base + 3u] = 1.0;
    } else {
        bright[base] = 0.0; bright[base + 1u] = 0.0;
        bright[base + 2u] = 0.0; bright[base + 3u] = 0.0;
    }
}
"#;

const HBLUR_WGSL: &str = r#"
struct Params { width: u32, height: u32, radius: u32, _pad: u32 }

@group(0) @binding(0) var<storage, read>        src:    array<f32>;
@group(0) @binding(1) var<storage, read_write>  dst:    array<f32>;
@group(0) @binding(2) var<storage, read>        kernel: array<f32>;
@group(0) @binding(3) var<uniform>              params: Params;

@compute @workgroup_size(64)
fn main(@builtin(global_invocation_id) id: vec3<u32>) {
    let i = id.x;
    if i >= params.width * params.height { return; }
    let x = i32(i % params.width);
    let y = i32(i / params.width);
    let w = i32(params.width);
    let base = i * 4u;
    var rv = src[base]      * kernel[0u];
    var gv = src[base + 1u] * kernel[0u];
    var bv = src[base + 2u] * kernel[0u];
    for (var k: u32 = 1u; k <= params.radius; k += 1u) {
        let xl = u32(max(x - i32(k), 0));
        let xr = u32(min(x + i32(k), w - 1));
        let il = (u32(y) * u32(w) + xl) * 4u;
        let ir = (u32(y) * u32(w) + xr) * 4u;
        let wt = kernel[k];
        rv += (src[il] + src[ir]) * wt;
        gv += (src[il + 1u] + src[ir + 1u]) * wt;
        bv += (src[il + 2u] + src[ir + 2u]) * wt;
    }
    dst[base] = rv; dst[base + 1u] = gv; dst[base + 2u] = bv; dst[base + 3u] = 1.0;
}
"#;

const VBLUR_COMPOSITE_WGSL: &str = r#"
struct Params { width: u32, height: u32, radius: u32, _pad: u32 }

@group(0) @binding(0) var<storage, read>        orig:   array<f32>;
@group(0) @binding(1) var<storage, read>         hblur:  array<f32>;
@group(0) @binding(2) var<storage, read_write>   dst:    array<f32>;
@group(0) @binding(3) var<storage, read>         kernel: array<f32>;
@group(0) @binding(4) var<uniform>               params: Params;

@compute @workgroup_size(64)
fn main(@builtin(global_invocation_id) id: vec3<u32>) {
    let i = id.x;
    if i >= params.width * params.height { return; }
    let x = i32(i % params.width);
    let y = i32(i / params.width);
    let w = i32(params.width);
    let h = i32(params.height);
    let base = i * 4u;
    var rv = hblur[base]      * kernel[0u];
    var gv = hblur[base + 1u] * kernel[0u];
    var bv = hblur[base + 2u] * kernel[0u];
    for (var k: u32 = 1u; k <= params.radius; k += 1u) {
        let yt = u32(max(y - i32(k), 0));
        let yb = u32(min(y + i32(k), h - 1));
        let it = (yt * u32(w) + u32(x)) * 4u;
        let ib = (yb * u32(w) + u32(x)) * 4u;
        let wt = kernel[k];
        rv += (hblur[it] + hblur[ib]) * wt;
        gv += (hblur[it + 1u] + hblur[ib + 1u]) * wt;
        bv += (hblur[it + 2u] + hblur[ib + 2u]) * wt;
    }
    let or_r = orig[base]; let or_g = orig[base + 1u];
    let or_b = orig[base + 2u]; let or_a = orig[base + 3u];
    if rv > 0.0 || gv > 0.0 || bv > 0.0 {
        dst[base]      = min(or_r + rv, 1.0);
        dst[base + 1u] = min(or_g + gv, 1.0);
        dst[base + 2u] = min(or_b + bv, 1.0);
        dst[base + 3u] = or_a;
    } else {
        dst[base]      = or_r; dst[base + 1u] = or_g;
        dst[base + 2u] = or_b; dst[base + 3u] = or_a;
    }
}
"#;

// ── Gaussian kernel (matches the CPU implementation in pmre-kit/post.rs) ─────

fn gaussian_kernel(sigma: f32, radius: usize) -> Vec<f32> {
    let mut k: Vec<f32> = (0..=radius)
        .map(|i| (-(i as f32 * i as f32) / (2.0 * sigma * sigma)).exp())
        .collect();
    let total = k[0] + 2.0 * k[1..].iter().sum::<f32>();
    for v in &mut k {
        *v /= total;
    }
    k
}

// ── GPU context (pipelines + device) ─────────────────────────────────────────

struct GpuContext {
    device: wgpu::Device,
    queue: wgpu::Queue,
    bright_pipe: wgpu::ComputePipeline,
    hblur_pipe: wgpu::ComputePipeline,
    vblur_composite_pipe: wgpu::ComputePipeline,
}

impl GpuContext {
    fn init() -> Option<Self> {
        pollster::block_on(Self::new_async())
    }

    async fn new_async() -> Option<Self> {
        let instance = wgpu::Instance::default();
        let adapter = instance
            .request_adapter(&wgpu::RequestAdapterOptions {
                power_preference: wgpu::PowerPreference::HighPerformance,
                force_fallback_adapter: false,
                compatible_surface: None,
            })
            .await?;
        let (device, queue) = adapter
            .request_device(&wgpu::DeviceDescriptor::default(), None)
            .await
            .ok()?;
        let bright_pipe = compile(&device, "bright_pass", BRIGHT_WGSL);
        let hblur_pipe = compile(&device, "hblur", HBLUR_WGSL);
        let vblur_composite_pipe = compile(&device, "vblur_composite", VBLUR_COMPOSITE_WGSL);
        Some(Self {
            device,
            queue,
            bright_pipe,
            hblur_pipe,
            vblur_composite_pipe,
        })
    }

    fn bloom(&self, fb: &mut Framebuffer, threshold: f32, sigma: f32, radius: usize) {
        let n = (fb.width * fb.height) as u64;
        let buf_size = n * 16; // 4 × f32 per pixel

        // Pixel data → raw bytes (r, g, b, a as little-endian f32s)
        let float_bytes: Vec<u8> = fb
            .pixels()
            .iter()
            .flat_map(|p| {
                let mut b = [0u8; 16];
                b[0..4].copy_from_slice(&p.r.to_le_bytes());
                b[4..8].copy_from_slice(&p.g.to_le_bytes());
                b[8..12].copy_from_slice(&p.b.to_le_bytes());
                b[12..16].copy_from_slice(&p.a.to_le_bytes());
                b
            })
            .collect();

        let src_buf = self
            .device
            .create_buffer_init(&wgpu::util::BufferInitDescriptor {
                label: Some("src"),
                contents: &float_bytes,
                usage: wgpu::BufferUsages::STORAGE,
            });

        let new_buf = |label: &str, extra: wgpu::BufferUsages| {
            self.device.create_buffer(&wgpu::BufferDescriptor {
                label: Some(label),
                size: buf_size,
                usage: wgpu::BufferUsages::STORAGE | extra,
                mapped_at_creation: false,
            })
        };
        let bright_buf = new_buf("bright", wgpu::BufferUsages::empty());
        let hblur_buf = new_buf("hblur", wgpu::BufferUsages::empty());
        let output_buf = new_buf("output", wgpu::BufferUsages::COPY_SRC);

        let staging = self.device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("staging"),
            size: buf_size,
            usage: wgpu::BufferUsages::MAP_READ | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });

        // Gaussian kernel as storage buffer
        let kernel = gaussian_kernel(sigma, radius);
        let kernel_bytes: Vec<u8> = kernel.iter().flat_map(|f| f.to_le_bytes()).collect();
        let kernel_buf = self
            .device
            .create_buffer_init(&wgpu::util::BufferInitDescriptor {
                label: Some("kernel"),
                contents: &kernel_bytes,
                usage: wgpu::BufferUsages::STORAGE,
            });

        // Uniform: [width, height, threshold, _pad]
        let bright_params = make_params_bytes([
            fb.width.to_le_bytes(),
            fb.height.to_le_bytes(),
            threshold.to_le_bytes(),
            0u32.to_le_bytes(),
        ]);
        let bright_params_buf = self
            .device
            .create_buffer_init(&wgpu::util::BufferInitDescriptor {
                label: Some("bright_params"),
                contents: &bright_params,
                usage: wgpu::BufferUsages::UNIFORM,
            });

        // Uniform: [width, height, radius, _pad]
        let blur_params = make_params_bytes([
            fb.width.to_le_bytes(),
            fb.height.to_le_bytes(),
            (radius as u32).to_le_bytes(),
            0u32.to_le_bytes(),
        ]);
        let blur_params_buf = self
            .device
            .create_buffer_init(&wgpu::util::BufferInitDescriptor {
                label: Some("blur_params"),
                contents: &blur_params,
                usage: wgpu::BufferUsages::UNIFORM,
            });

        // Bind groups
        let bright_bg = self.device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("bright_bg"),
            layout: &self.bright_pipe.get_bind_group_layout(0),
            entries: &[
                bge(0, src_buf.as_entire_binding()),
                bge(1, bright_buf.as_entire_binding()),
                bge(2, bright_params_buf.as_entire_binding()),
            ],
        });
        let hblur_bg = self.device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("hblur_bg"),
            layout: &self.hblur_pipe.get_bind_group_layout(0),
            entries: &[
                bge(0, bright_buf.as_entire_binding()),
                bge(1, hblur_buf.as_entire_binding()),
                bge(2, kernel_buf.as_entire_binding()),
                bge(3, blur_params_buf.as_entire_binding()),
            ],
        });
        let vblur_bg = self.device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("vblur_bg"),
            layout: &self.vblur_composite_pipe.get_bind_group_layout(0),
            entries: &[
                bge(0, src_buf.as_entire_binding()),
                bge(1, hblur_buf.as_entire_binding()),
                bge(2, output_buf.as_entire_binding()),
                bge(3, kernel_buf.as_entire_binding()),
                bge(4, blur_params_buf.as_entire_binding()),
            ],
        });

        // Encode three compute passes + copy to staging
        let workgroups = (n as u32).div_ceil(64);
        let mut enc = self
            .device
            .create_command_encoder(&wgpu::CommandEncoderDescriptor {
                label: Some("bloom"),
            });
        {
            let mut p = enc.begin_compute_pass(&wgpu::ComputePassDescriptor {
                label: Some("bright_pass"),
                timestamp_writes: None,
            });
            p.set_pipeline(&self.bright_pipe);
            p.set_bind_group(0, &bright_bg, &[]);
            p.dispatch_workgroups(workgroups, 1, 1);
        }
        {
            let mut p = enc.begin_compute_pass(&wgpu::ComputePassDescriptor {
                label: Some("hblur"),
                timestamp_writes: None,
            });
            p.set_pipeline(&self.hblur_pipe);
            p.set_bind_group(0, &hblur_bg, &[]);
            p.dispatch_workgroups(workgroups, 1, 1);
        }
        {
            let mut p = enc.begin_compute_pass(&wgpu::ComputePassDescriptor {
                label: Some("vblur_composite"),
                timestamp_writes: None,
            });
            p.set_pipeline(&self.vblur_composite_pipe);
            p.set_bind_group(0, &vblur_bg, &[]);
            p.dispatch_workgroups(workgroups, 1, 1);
        }
        enc.copy_buffer_to_buffer(&output_buf, 0, &staging, 0, buf_size);
        self.queue.submit([enc.finish()]);

        // Wait for the GPU to finish, then read back
        let slice = staging.slice(..);
        let (tx, rx) = std::sync::mpsc::channel();
        slice.map_async(wgpu::MapMode::Read, move |r| {
            let _ = tx.send(r);
        });
        self.device.poll(wgpu::Maintain::Wait);
        if rx.recv().ok().and_then(|r| r.ok()).is_none() {
            return;
        }
        {
            let data = slice.get_mapped_range();
            let pixels = fb.pixels_mut();
            for (i, chunk) in data.chunks_exact(16).enumerate() {
                pixels[i] = Rgba::new(
                    f32::from_le_bytes([chunk[0], chunk[1], chunk[2], chunk[3]]),
                    f32::from_le_bytes([chunk[4], chunk[5], chunk[6], chunk[7]]),
                    f32::from_le_bytes([chunk[8], chunk[9], chunk[10], chunk[11]]),
                    f32::from_le_bytes([chunk[12], chunk[13], chunk[14], chunk[15]]),
                );
            }
        }
        staging.unmap();
    }
}

// ── Helpers ───────────────────────────────────────────────────────────────────

fn compile(device: &wgpu::Device, label: &str, wgsl: &str) -> wgpu::ComputePipeline {
    let module = device.create_shader_module(wgpu::ShaderModuleDescriptor {
        label: Some(label),
        source: wgpu::ShaderSource::Wgsl(wgsl.to_string().into()),
    });
    device.create_compute_pipeline(&wgpu::ComputePipelineDescriptor {
        label: Some(label),
        layout: None,
        module: &module,
        entry_point: "main",
        compilation_options: wgpu::PipelineCompilationOptions::default(),
        cache: None,
    })
}

fn bge(binding: u32, resource: wgpu::BindingResource<'_>) -> wgpu::BindGroupEntry<'_> {
    wgpu::BindGroupEntry { binding, resource }
}

fn make_params_bytes(fields: [[u8; 4]; 4]) -> [u8; 16] {
    let mut out = [0u8; 16];
    out[0..4].copy_from_slice(&fields[0]);
    out[4..8].copy_from_slice(&fields[1]);
    out[8..12].copy_from_slice(&fields[2]);
    out[12..16].copy_from_slice(&fields[3]);
    out
}

// ── Public entry point ────────────────────────────────────────────────────────

thread_local! {
    static GPU: Option<GpuContext> = GpuContext::init();
}

/// Run GPU-accelerated Gaussian bloom on `fb`. Falls back to the CPU path if
/// wgpu initialisation failed (no suitable adapter, or driver unsupported).
pub fn gpu_bloom(fb: &mut Framebuffer, threshold: f32, sigma: f32, radius: usize) {
    GPU.with(|ctx| match ctx {
        Some(c) => c.bloom(fb, threshold, sigma, radius),
        None => post::bloom(fb, threshold, sigma, radius),
    });
}

/// Returns the wgpu backend name for the GPU acquired on this thread, or `"cpu-fallback"`
/// if GPU initialisation failed.
pub fn gpu_backend_name() -> &'static str {
    GPU.with(|ctx| {
        if ctx.is_some() {
            "wgpu-gpu"
        } else {
            "cpu-fallback"
        }
    })
}
