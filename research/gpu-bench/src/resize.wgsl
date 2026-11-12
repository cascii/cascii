struct Params { sw: u32, sh: u32, dw: u32, dh: u32, threshold: u32, count: u32, pad0: u32, pad1: u32 }
@group(0) @binding(0) var<storage, read> source: array<u32>;
@group(0) @binding(1) var<storage, read_write> intermediate: array<vec4<f32>>;
@group(0) @binding(2) var<storage, read_write> output: array<u32>;
@group(0) @binding(3) var<uniform> p: Params;
@group(0) @binding(4) var<storage, read> ramp: array<u32>;
fn byte_at(i: u32) -> u32 { return (source[i / 4u] >> ((i % 4u) * 8u)) & 255u; }
fn pixel(i: u32) -> vec3<f32> { return vec3<f32>(f32(byte_at(i*3u)), f32(byte_at(i*3u+1u)), f32(byte_at(i*3u+2u))); }
fn weight(i: u32, center: f32, scale: f32) -> f32 { return max(0.0, 1.0 - abs((f32(i) - center) / scale)); }
@compute @workgroup_size(16, 16)
fn vertical(@builtin(global_invocation_id) id: vec3<u32>) {
    if (id.x >= p.sw || id.y >= p.dh) { return; }
    let ratio = f32(p.sh) / f32(p.dh);
    let scale = max(ratio, 1.0);
    let center = (f32(id.y) + 0.5) * ratio;
    let left = u32(clamp(floor(center-scale), 0.0, f32(p.sh-1u)));
    let right = u32(clamp(ceil(center+scale), f32(left+1u), f32(p.sh)));
    var sum = 0.0;
    for (var i=left; i<right; i++) { sum += weight(i, center-0.5, scale); }
    var rgb = vec3<f32>(0.0);
    for (var i=left; i<right; i++) { rgb += pixel(i*p.sw+id.x) * (weight(i, center-0.5, scale)/sum); }
    intermediate[id.y*p.sw+id.x] = vec4<f32>(rgb, 0.0);
}
@compute @workgroup_size(16, 16)
fn horizontal(@builtin(global_invocation_id) id: vec3<u32>) {
    if (id.x >= p.dw || id.y >= p.dh) { return; }
    let ratio = f32(p.sw) / f32(p.dw);
    let scale = max(ratio, 1.0);
    let center = (f32(id.x) + 0.5) * ratio;
    let left = u32(clamp(floor(center-scale), 0.0, f32(p.sw-1u)));
    let right = u32(clamp(ceil(center+scale), f32(left+1u), f32(p.sw)));
    var sum = 0.0;
    for (var i=left; i<right; i++) { sum += weight(i, center-0.5, scale); }
    var rgb = vec3<f32>(0.0);
    for (var i=left; i<right; i++) { rgb += intermediate[id.y*p.sw+i].xyz * (weight(i, center-0.5, scale)/sum); }
    let c = vec3<u32>(floor(clamp(rgb, vec3<f32>(0.0), vec3<f32>(255.0))+vec3<f32>(0.5)));
    let luma = (2126u*c.x + 7152u*c.y + 722u*c.z)/10000u;
    var ch = 32u;
    if (luma >= p.threshold) { ch = ramp[min((luma-p.threshold)*(p.count-1u)/max(255u-p.threshold,1u),p.count-1u)]; }
    output[id.y*p.dw+id.x] = ch | (c.x << 8u) | (c.y << 16u) | (c.z << 24u);
}
