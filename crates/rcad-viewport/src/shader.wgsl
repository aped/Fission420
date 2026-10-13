struct Uniforms {
    view_proj: mat4x4<f32>,
    eye: vec4<f32>,
    light_dir: vec4<f32>,
    base_color: vec4<f32>,
    highlight_color: vec4<f32>,
    selection_color: vec4<f32>,
    // rgb = linear colour to fade toward, a = fade amount (0 = off)
    fade: vec4<f32>,
    // Section view (display only): xyz = unit normal, w = offset; material
    // with dot(p, n) > w is hidden. Active when highlight.z == 1.
    section: vec4<f32>,
    // x = highlighted group id, y = 1 if highlight active, z = 1 if section on
    highlight: vec4<u32>,
};

// Inside surfaces exposed by an (uncapped) section view.
const SECTION_INSIDE: vec3<f32> = vec3<f32>(0.62, 0.30, 0.26);

@group(0) @binding(0) var<uniform> u: Uniforms;

const FLAG_SELECTED: u32 = 1u;
// Flag bits 8..11: body tint index (0 = u.base_color). Linear RGB.
const TINTS = array<vec3<f32>, 8>(
    vec3<f32>(0.42, 0.52, 0.66),
    vec3<f32>(0.42, 0.52, 0.66),
    vec3<f32>(0.40, 0.62, 0.45),
    vec3<f32>(0.66, 0.50, 0.62),
    vec3<f32>(0.62, 0.58, 0.40),
    vec3<f32>(0.40, 0.62, 0.66),
    vec3<f32>(0.66, 0.46, 0.42),
    vec3<f32>(0.62, 0.60, 0.56),
);
// Lines are pulled toward the eye by this fraction of their eye distance
// (3 µm at 300 mm). The main defence against coplanar z-fighting is the
// mesh pipeline's depth bias (ULP-relative constant + slope), which WebGPU
// does not allow on line topologies; this is a small extra margin.
const LINE_EYE_NUDGE: f32 = 1.0e-5;

struct MeshOut {
    @builtin(position) clip: vec4<f32>,
    @location(0) world: vec3<f32>,
    @location(1) normal: vec3<f32>,
    @location(2) @interpolate(flat) group: u32,
    @location(3) @interpolate(flat) flags: u32,
};

@vertex
fn vs_mesh(
    @location(0) pos: vec3<f32>,
    @location(1) normal: vec3<f32>,
    @location(2) group: u32,
    @location(3) flags: u32,
) -> MeshOut {
    var o: MeshOut;
    o.clip = u.view_proj * vec4<f32>(pos, 1.0);
    o.world = pos;
    o.normal = normal;
    o.group = group;
    o.flags = flags;
    return o;
}

// Output is gamma-encoded: egui samples user textures as sRGB-gamma values.
fn to_gamma(c: vec3<f32>) -> vec3<f32> {
    return pow(clamp(c, vec3<f32>(0.0), vec3<f32>(1.0)), vec3<f32>(1.0 / 2.2));
}

@fragment
fn fs_mesh(i: MeshOut) -> @location(0) vec4<f32> {
    let section = u.highlight.z == 1u;
    if section && dot(i.world, u.section.xyz) > u.section.w {
        discard;
    }
    var n = normalize(i.normal);
    let v = normalize(u.eye.xyz - i.world);
    // Two-sided lighting. With a section, a face seen from behind is the
    // inside of the material (the cut is not capped).
    let inside = dot(n, v) < 0.0;
    if inside {
        n = -n;
    }
    let l = u.light_dir.xyz;
    let diffuse = max(dot(n, l), 0.0);
    // Small fill light from the view direction so back-lit faces are not flat.
    let fill = max(dot(n, v), 0.0);
    var base = u.base_color.rgb;
    let tint = (i.flags >> 8u) & 7u;
    if tint != 0u {
        var tints = TINTS;
        base = tints[tint];
    }
    if (i.flags & FLAG_SELECTED) != 0u {
        base = u.selection_color.rgb;
    }
    if section && inside {
        base = SECTION_INSIDE;
    }
    // Hover wins over selection.
    if u.highlight.y == 1u && i.group == u.highlight.x {
        base = u.highlight_color.rgb;
    }
    var lin = base * (0.12 + 0.68 * diffuse + 0.20 * fill);
    if u.fade.a > 0.0 {
        // Desaturate, then blend toward the background.
        let lum = dot(lin, vec3<f32>(0.2126, 0.7152, 0.0722));
        lin = mix(lin, vec3<f32>(lum), 0.6);
        lin = mix(lin, u.fade.rgb, u.fade.a);
    }
    return vec4<f32>(to_gamma(lin), 1.0);
}

struct LineOut {
    @builtin(position) clip: vec4<f32>,
    @location(0) color: vec4<f32>,
};

@vertex
fn vs_line(@location(0) pos: vec3<f32>, @location(1) color: vec4<f32>) -> LineOut {
    var o: LineOut;
    let p = mix(pos, u.eye.xyz, LINE_EYE_NUDGE);
    o.clip = u.view_proj * vec4<f32>(p, 1.0);
    o.color = color;
    return o;
}

@fragment
fn fs_line(i: LineOut) -> @location(0) vec4<f32> {
    return i.color;
}
