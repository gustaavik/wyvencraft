#version 450

layout(location = 0) in vec2 v_uv;
layout(location = 1) in float v_ao;
layout(location = 2) in vec3 v_normal;
layout(location = 3) flat in uint v_flags;

layout(set = 0, binding = 0) uniform sampler2D atlas;

// Shared with voxel.vert — layout must stay identical across both stages.
layout(push_constant) uniform PushConstants {
    mat4 view_proj;
    vec4 sun_dir;     // xyz: direction toward the light, w: time in seconds
    vec4 light_color; // rgb: directional light tint, w: ambient floor
} pc;

layout(location = 0) out vec4 f_color;

// Must match render::vertex::FLAG_WATER.
const uint FLAG_WATER = 1u;
// Must match render::texture::ATLAS_COLUMNS and render::tiles::WATER_FRAMES.
const float ATLAS_COLUMNS = 16.0;
const float WATER_FRAMES = 4.0;
const float WATER_FPS = 5.0;

void main() {
    vec2 uv = v_uv;
    // Water animation: step through the frames laid out to the right of the
    // base water tile in the atlas.
    if ((v_flags & FLAG_WATER) != 0u) {
        float frame = floor(mod(pc.sun_dir.w * WATER_FPS, WATER_FRAMES));
        uv.x += frame / ATLAS_COLUMNS;
    }
    vec4 tex = texture(atlas, uv);
    // Alpha-test for cutout foliage / sprites in the opaque pass.
    if (tex.a < 0.1) {
        discard;
    }
    // Baked face shade (v_ao) plus a time-of-day directional term: ambient floor
    // lifted by the sun/moon contribution, then tinted by the light color.
    float ambient = pc.light_color.w;
    float ndl = max(dot(normalize(v_normal), normalize(pc.sun_dir.xyz)), 0.0);
    float diffuse = ambient + (1.0 - ambient) * ndl;
    f_color = vec4(tex.rgb * v_ao * diffuse * pc.light_color.rgb, tex.a);
}
