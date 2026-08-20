// Minimal DCP render shader for GPU-vs-CPU parity testing (W3).
// Only the DCP stage — no legacy adjustments, masks, LUTs, or tonemapping.
// Bindings: 0=input (rgba16float 2D), 1=output (rgba8unorm 2D),
// 2=uniforms (DcpParityUniforms), 3=huesat_3d, 4=look_3d, 5=tone_curve_1d.

struct DcpParityUniforms {
    is_raw_image: u32,
    has_dcp: u32,
    _pad_a: u32,
    _pad_b: u32,
    dcp_huesat_dims: vec4<u32>,
    dcp_look_dims: vec4<u32>,
    cam_to_prophoto: mat3x3<f32>,
    prophoto_to_working: mat3x3<f32>,
}

@group(0) @binding(0) var input_texture: texture_2d<f32>;
@group(0) @binding(1) var output_texture: texture_storage_2d<rgba8unorm, write>;
@group(0) @binding(2) var<storage, read> u: DcpParityUniforms;
@group(0) @binding(3) var dcp_huesat_tex: texture_3d<f32>;
@group(0) @binding(4) var dcp_look_tex: texture_3d<f32>;
@group(0) @binding(5) var dcp_tone_curve_tex: texture_1d<f32>;

// ---- HSV conversion (matching shader.wgsl) --------------------------------

fn rgb_to_hsv(c: vec3<f32>) -> vec3<f32> {
    let c_max = max(c.r, max(c.g, c.b));
    let c_min = min(c.r, min(c.g, c.b));
    let delta = c_max - c_min;
    var h: f32 = 0.0;
    if (delta > 0.0) {
        if (c_max == c.r) { h = 60.0 * (((c.g - c.b) / delta) % 6.0); }
        else if (c_max == c.g) { h = 60.0 * (((c.b - c.r) / delta) + 2.0); }
        else { h = 60.0 * (((c.r - c.g) / delta) + 4.0); }
    }
    if (h < 0.0) { h += 360.0; }
    let s = select(0.0, delta / c_max, c_max > 0.0);
    return vec3<f32>(h, s, c_max);
}

fn hsv_to_rgb(c: vec3<f32>) -> vec3<f32> {
    let h = c.x; let s = c.y; let v = c.z;
    let C = v * s;
    let X = C * (1.0 - abs((h / 60.0) % 2.0 - 1.0));
    let m = v - C;
    var rgb_prime: vec3<f32>;
    if (h < 60.0) { rgb_prime = vec3<f32>(C, X, 0.0); }
    else if (h < 120.0) { rgb_prime = vec3<f32>(X, C, 0.0); }
    else if (h < 180.0) { rgb_prime = vec3<f32>(0.0, C, X); }
    else if (h < 240.0) { rgb_prime = vec3<f32>(0.0, X, C); }
    else if (h < 300.0) { rgb_prime = vec3<f32>(X, 0.0, C); }
    else { rgb_prime = vec3<f32>(C, 0.0, X); }
    return rgb_prime + vec3<f32>(m, m, m);
}

// ---- DCP sign-symmetric sRGB transfer -------------------------------------

fn dcp_linear_to_srgb_ext(x: f32) -> f32 {
    let a = abs(x);
    let lower = a * 12.92;
    let higher = 1.055 * pow(a, 1.0 / 2.4) - 0.055;
    let v = select(higher, lower, a <= 0.0031308);
    return sign(x) * v;
}

fn dcp_srgb_to_linear_ext(x: f32) -> f32 {
    let a = abs(x);
    let lower = a / 12.92;
    let higher = pow((a + 0.055) / 1.055, 2.4);
    let v = select(higher, lower, a <= 0.04045);
    return sign(x) * v;
}

// ---- DCP table fetching (manual trilinear, hue-wrapping) -------------------

fn dcp_table_cell(table: texture_3d<f32>, h: u32, s: u32, v: u32, hue_dim: u32) -> vec3<f32> {
    return textureLoad(table, vec3<i32>(i32(h % hue_dim), i32(s), i32(v)), 0).xyz;
}

fn dcp_sample_table(
    table: texture_3d<f32>,
    hue_dim: u32, sat_dim: u32, val_dim: u32,
    hue_coord: f32, sat_coord: f32, val_coord: f32,
) -> vec3<f32> {
    let h_fl = floor(hue_coord);
    let fh = hue_coord - h_fl;
    let h0 = u32(h_fl) % hue_dim;
    let h1 = (h0 + 1u) % hue_dim;

    let sf = clamp(sat_coord * f32(sat_dim - 1u), 0.0, f32(sat_dim - 1u));
    let s0 = u32(floor(sf));
    let s1 = min(s0 + 1u, sat_dim - 1u);
    let fs = sf - f32(s0);

    var v0: u32 = 0u; var v1: u32 = 0u; var fv: f32 = 0.0;
    if val_dim > 1u {
        let vf = clamp(val_coord * f32(val_dim - 1u), 0.0, f32(val_dim - 1u));
        v0 = u32(floor(vf));
        v1 = min(v0 + 1u, val_dim - 1u);
        fv = vf - f32(v0);
    }

    let c000 = dcp_table_cell(table, h0, s0, v0, hue_dim);
    let c100 = dcp_table_cell(table, h1, s0, v0, hue_dim);
    let c010 = dcp_table_cell(table, h0, s1, v0, hue_dim);
    let c110 = dcp_table_cell(table, h1, s1, v0, hue_dim);
    let c001 = dcp_table_cell(table, h0, s0, v1, hue_dim);
    let c101 = dcp_table_cell(table, h1, s0, v1, hue_dim);
    let c011 = dcp_table_cell(table, h0, s1, v1, hue_dim);
    let c111 = dcp_table_cell(table, h1, s1, v1, hue_dim);

    let w000 = (1.0 - fh) * (1.0 - fs) * (1.0 - fv);
    let w100 = fh * (1.0 - fs) * (1.0 - fv);
    let w010 = (1.0 - fh) * fs * (1.0 - fv);
    let w110 = fh * fs * (1.0 - fv);
    let w001 = (1.0 - fh) * (1.0 - fs) * fv;
    let w101 = fh * (1.0 - fs) * fv;
    let w011 = (1.0 - fh) * fs * fv;
    let w111 = fh * fs * fv;

    return c000 * w000 + c100 * w100 + c010 * w010 + c110 * w110
         + c001 * w001 + c101 * w101 + c011 * w011 + c111 * w111;
}

fn dcp_apply_hsv_table(
    rgb: vec3<f32>, table: texture_3d<f32>, dims: vec4<u32>, encoding: u32,
) -> vec3<f32> {
    var rgb_enc = rgb;
    if encoding == 1u {
        rgb_enc = vec3<f32>(dcp_linear_to_srgb_ext(rgb.r),
                            dcp_linear_to_srgb_ext(rgb.g),
                            dcp_linear_to_srgb_ext(rgb.b));
    }
    let hsv = rgb_to_hsv(rgb_enc);
    let hue_coord = (hsv.x / 360.0) * f32(dims.x);
    let sat_coord = hsv.y;
    let val_coord = select(0.0, hsv.z, dims.z > 1u);
    let entry = dcp_sample_table(table, dims.x, dims.y, dims.z,
                                  hue_coord, sat_coord, val_coord);
    let h2 = (hsv.x + entry.x) % 360.0;
    let s2 = clamp(hsv.y * entry.y, 0.0, 1.0);
    let v2 = hsv.z * entry.z;
    let rgb2 = hsv_to_rgb(vec3<f32>(h2, s2, v2));
    if encoding == 1u {
        return vec3<f32>(dcp_srgb_to_linear_ext(rgb2.r),
                         dcp_srgb_to_linear_ext(rgb2.g),
                         dcp_srgb_to_linear_ext(rgb2.b));
    }
    return rgb2;
}

fn dcp_eval_tone_curve(lut: texture_1d<f32>, x: f32) -> f32 {
    let lut_len = 4096u;
    if x <= 0.0 {
        let y0 = textureLoad(lut, vec2<i32>(0, 0), 0).x;
        let y1 = textureLoad(lut, vec2<i32>(1, 0), 0).x;
        return max(y0 + x * (y1 - y0) * f32(lut_len - 1u), 0.0);
    }
    if x >= 1.0 {
        let last = i32(lut_len - 1u);
        let yl = textureLoad(lut, vec2<i32>(last, 0), 0).x;
        let yp = textureLoad(lut, vec2<i32>(last - 1, 0), 0).x;
        return max(yl + (x - 1.0) * (yl - yp) * f32(lut_len - 1u), 0.0);
    }
    let f = x * f32(lut_len - 1u);
    let i = u32(floor(f));
    let t = f - f32(i);
    let idx = i32(min(i, lut_len - 2u));
    let v0 = textureLoad(lut, vec2<i32>(idx, 0), 0).x;
    let v1 = textureLoad(lut, vec2<i32>(idx + 1, 0), 0).x;
    return v0 + (v1 - v0) * t;
}

fn dcp_apply_tone_curve(rgb: vec3<f32>, lut: texture_1d<f32>) -> vec3<f32> {
    return vec3<f32>(dcp_eval_tone_curve(lut, rgb.r),
                     dcp_eval_tone_curve(lut, rgb.g),
                     dcp_eval_tone_curve(lut, rgb.b));
}

// ---- Entry point -----------------------------------------------------------

@compute @workgroup_size(8, 8, 1)
fn main(@builtin(global_invocation_id) id: vec3<u32>) {
    let out_dims = vec2<u32>(textureDimensions(output_texture));
    if (id.x >= out_dims.x || id.y >= out_dims.y) { return; }

    // Load camera-native linear RGB.
    var rgb = textureLoad(input_texture, id.xy, 0).rgb;

    if (u.has_dcp != 0u) {
        // Camera → ProPhoto (D50).
        rgb = u.cam_to_prophoto * rgb;

        // HueSatMap (if present).
        if u.dcp_huesat_dims.x > 0u {
            rgb = dcp_apply_hsv_table(rgb, dcp_huesat_tex, u.dcp_huesat_dims, 0u);
        }

        // LookTable (if present).
        if u.dcp_look_dims.x > 0u {
            rgb = dcp_apply_hsv_table(rgb, dcp_look_tex, u.dcp_look_dims, 0u);
        }

        // Tone curve.
        rgb = dcp_apply_tone_curve(rgb, dcp_tone_curve_tex);

        // ProPhoto → working space.
        rgb = u.prophoto_to_working * rgb;
    }

    // Clamp and store (no tonemapping — raw linear values may exceed 1.0).
    let safe = clamp(rgb, vec3<f32>(0.0), vec3<f32>(1.0));
    textureStore(output_texture, id.xy, vec4<f32>(safe, 1.0));
}
