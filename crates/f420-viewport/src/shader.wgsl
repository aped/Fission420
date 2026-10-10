struct Uniforms {
    view_proj: mat4x4<f32>,
    eye: vec4<f32>,
    light_dir: vec4<f32>,
    base_color: vec4<f32>,
    highlight_color: vec4<f32>,
    // x = highlighted group id, y = 1 if highlight active
    highlight: vec4<u32>,
};

@group(0) @binding(0) var<uniform> u: Uniforms;

struct MeshOut {
    @builtin(position) clip: vec4<f32>,
    @location(0) world: vec3<f32>,
    @location(1) normal: vec3<f32>,
    @location(2) @interpolate(flat) group: u32,
};

@vertex
fn vs_mesh(
    @location(0) pos: vec3<f32>,
    @location(1) normal: vec3<f32>,
    @location(2) group: u32,
) -> MeshOut {
    var o: MeshOut;
    o.clip = u.view_proj * vec4<f32>(pos, 1.0);
    o.world = pos;
    o.normal = normal;
    o.group = group;
    return o;
}

// Output is gamma-encoded: egui samples user textures as sRGB-gamma values.
fn to_gamma(c: vec3<f32>) -> vec3<f32> {
    return pow(clamp(c, vec3<f32>(0.0), vec3<f32>(1.0)), vec3<f32>(1.0 / 2.2));
}

@fragment
fn fs_mesh(i: MeshOut) -> @location(0) vec4<f32> {
    var n = normalize(i.normal);
    let v = normalize(u.eye.xyz - i.world);
    // Two-sided lighting.
    if dot(n, v) < 0.0 {
        n = -n;
    }
    let l = u.light_dir.xyz;
    let diffuse = max(dot(n, l), 0.0);
    // Small fill light from the view direction so back-lit faces are not flat.
    let fill = max(dot(n, v), 0.0);
    var base = u.base_color.rgb;
    if u.highlight.y == 1u && i.group == u.highlight.x {
        base = u.highlight_color.rgb;
    }
    let lin = base * (0.12 + 0.68 * diffuse + 0.20 * fill);
    return vec4<f32>(to_gamma(lin), 1.0);
}

struct LineOut {
    @builtin(position) clip: vec4<f32>,
    @location(0) color: vec4<f32>,
};

@vertex
fn vs_line(@location(0) pos: vec3<f32>, @location(1) color: vec4<f32>) -> LineOut {
    var o: LineOut;
    o.clip = u.view_proj * vec4<f32>(pos, 1.0);
    o.color = color;
    return o;
}

@fragment
fn fs_line(i: LineOut) -> @location(0) vec4<f32> {
    return i.color;
}
