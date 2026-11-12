use anyhow::{ensure, Context, Result};
use wgpu::util::DeviceExt;

pub struct Gpu {
    device: wgpu::Device,
    queue: wgpu::Queue,
    layout: wgpu::BindGroupLayout,
    vertical: wgpu::ComputePipeline,
    horizontal: wgpu::ComputePipeline,
    pub adapter: String,
}
pub struct Plan {
    input: wgpu::Buffer,
    output: wgpu::Buffer,
    readback: wgpu::Buffer,
    bind: wgpu::BindGroup,
    sw: u32,
    sh: u32,
    dw: u32,
    dh: u32,
}
impl Gpu {
    pub fn new() -> Result<Self> {
        let instance = wgpu::Instance::new(wgpu::InstanceDescriptor { backends: wgpu::Backends::VULKAN, ..Default::default() });
        let adapter = pollster::block_on(instance.request_adapter(&wgpu::RequestAdapterOptions { power_preference: wgpu::PowerPreference::HighPerformance, compatible_surface: None, force_fallback_adapter: false })).context("No Vulkan GPU adapter")?;
        let info = adapter.get_info();
        ensure!(info.device_type != wgpu::DeviceType::Cpu, "Refusing software adapter");
        let (device, queue) = pollster::block_on(adapter.request_device(&wgpu::DeviceDescriptor { label: Some("cascii research"), required_features: wgpu::Features::empty(), required_limits: wgpu::Limits::default() }, None))?;
        let entries: Vec<_> = (0..5).map(|binding| wgpu::BindGroupLayoutEntry { binding, visibility: wgpu::ShaderStages::COMPUTE, ty: wgpu::BindingType::Buffer { ty: if binding == 3 { wgpu::BufferBindingType::Uniform } else { wgpu::BufferBindingType::Storage { read_only: binding == 0 || binding == 4 } }, has_dynamic_offset: false, min_binding_size: None }, count: None }).collect();
        let layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor { label: None, entries: &entries });
        let pl = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor { label: None, bind_group_layouts: &[&layout], push_constant_ranges: &[] });
        let shader = device.create_shader_module(wgpu::ShaderModuleDescriptor { label: Some("triangle + ASCII"), source: wgpu::ShaderSource::Wgsl(include_str!("resize.wgsl").into()) });
        let pipeline = |entry_point| device.create_compute_pipeline(&wgpu::ComputePipelineDescriptor { label: Some(entry_point), layout: Some(&pl), module: &shader, entry_point });
        let vertical = pipeline("vertical");
        let horizontal = pipeline("horizontal");
        Ok(Self { device, queue, layout, vertical, horizontal, adapter: format!("{:?}", info) })
    }
    pub fn plan(&self, sw: u32, sh: u32, dw: u32, dh: u32, threshold: u8, chars: &[u8]) -> Result<Plan> {
        ensure!(sw > 0 && sh > 0 && dw > 0 && dh > 0 && !chars.is_empty() && chars.iter().all(|b| (32..=126).contains(b)), "invalid dimensions or ramp");
        let input_size = (u64::from(sw) * u64::from(sh) * 3 + 3) / 4 * 4;
        let intermediate_size = u64::from(sw) * u64::from(dh) * 16;
        let out_size = u64::from(dw) * u64::from(dh) * 4;
        let limit = u64::from(self.device.limits().max_storage_buffer_binding_size);
        ensure!([input_size, intermediate_size, out_size].iter().all(|&n| n <= limit), "buffer exceeds adapter binding limit");
        ensure!([sw.div_ceil(16), dw.div_ceil(16), dh.div_ceil(16)].iter().all(|&n| n <= self.device.limits().max_compute_workgroups_per_dimension), "dispatch too large");
        let buffer = |size, usage| self.device.create_buffer(&wgpu::BufferDescriptor { label: None, size, usage, mapped_at_creation: false });
        let input = buffer(input_size, wgpu::BufferUsages::STORAGE | wgpu::BufferUsages::COPY_DST);
        let intermediate = buffer(intermediate_size, wgpu::BufferUsages::STORAGE);
        let output = buffer(out_size, wgpu::BufferUsages::STORAGE | wgpu::BufferUsages::COPY_SRC);
        let readback = buffer(out_size, wgpu::BufferUsages::MAP_READ | wgpu::BufferUsages::COPY_DST);
        let params = [sw, sh, dw, dh, u32::from(threshold), chars.len() as u32, 0, 0];
        let uniform = self.device.create_buffer_init(&wgpu::util::BufferInitDescriptor { label: None, contents: bytemuck::cast_slice(&params), usage: wgpu::BufferUsages::UNIFORM });
        let chars: Vec<u32> = chars.iter().map(|&b| u32::from(b)).collect();
        let ramp = self.device.create_buffer_init(&wgpu::util::BufferInitDescriptor { label: None, contents: bytemuck::cast_slice(&chars), usage: wgpu::BufferUsages::STORAGE });
        let buffers = [&input, &intermediate, &output, &uniform, &ramp];
        let entries: Vec<_> = buffers.iter().enumerate().map(|(i, b)| wgpu::BindGroupEntry { binding: i as u32, resource: b.as_entire_binding() }).collect();
        let bind = self.device.create_bind_group(&wgpu::BindGroupDescriptor { label: None, layout: &self.layout, entries: &entries });
        Ok(Plan { input, output, readback, bind, sw, sh, dw, dh })
    }
    // Wall time includes host upload, both dispatches, fence, readback and cframe materialization.
    pub fn convert(&self, plan: &Plan, rgb: &[u8]) -> Result<Vec<u8>> {
        ensure!(rgb.len() == plan.sw as usize * plan.sh as usize * 3, "input byte length mismatch");
        // Queue writes require four-byte alignment. Upload the aligned prefix and
        // at most three tail bytes rather than cloning an entire odd-sized image.
        let aligned = rgb.len() / 4 * 4;
        if aligned > 0 {
            self.queue.write_buffer(&plan.input, 0, &rgb[..aligned]);
        }
        if aligned < rgb.len() {
            let mut tail = [0u8; 4];
            tail[..rgb.len() - aligned].copy_from_slice(&rgb[aligned..]);
            self.queue.write_buffer(&plan.input, aligned as u64, &tail);
        }
        let mut encoder = self.device.create_command_encoder(&wgpu::CommandEncoderDescriptor { label: None });
        {
            let mut pass = encoder.begin_compute_pass(&wgpu::ComputePassDescriptor { label: None, timestamp_writes: None });
            pass.set_bind_group(0, &plan.bind, &[]);
            pass.set_pipeline(&self.vertical);
            pass.dispatch_workgroups(plan.sw.div_ceil(16), plan.dh.div_ceil(16), 1);
            pass.set_pipeline(&self.horizontal);
            pass.dispatch_workgroups(plan.dw.div_ceil(16), plan.dh.div_ceil(16), 1);
        }
        encoder.copy_buffer_to_buffer(&plan.output, 0, &plan.readback, 0, u64::from(plan.dw) * u64::from(plan.dh) * 4);
        self.queue.submit(Some(encoder.finish()));
        let slice = plan.readback.slice(..);
        let (tx, rx) = std::sync::mpsc::channel();
        slice.map_async(wgpu::MapMode::Read, move |r| {
            let _ = tx.send(r);
        });
        self.device.poll(wgpu::Maintain::Wait);
        rx.recv()??;
        let mut bytes = Vec::with_capacity(8 + plan.dw as usize * plan.dh as usize * 4);
        bytes.extend_from_slice(&plan.dw.to_le_bytes());
        bytes.extend_from_slice(&plan.dh.to_le_bytes());
        {
            let mapped = slice.get_mapped_range();
            bytes.extend_from_slice(&mapped);
        }
        plan.readback.unmap();
        Ok(bytes)
    }
}
