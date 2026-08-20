#version 450

// Blocks authored in Blockbench sample render::block_textures — one 256x256
// array layer per texture, chosen per vertex — instead of the shared 16px
// atlas voxel.frag reads. Same vertex shader, same push constants; only the
// texture binding and the tint differ.

layout(location = 0) in vec2 v_uv;
layout(location = 1) in float v_ao;
layout(location = 2) in vec3 v_normal;
layout(location = 3) flat in uint v_flags;
layout(location = 4) flat in uint v_layer;
layout(location = 5) in vec4 v_tint;

layout(set = 0, binding = 0) uniform sampler2DArray block_textures;

// Shared with voxel.vert — layout must stay identical across both stages.
layout(push_constant) uniform PushConstants {
    mat4 view_proj;
    vec4 sun_dir;     // xyz: direction toward the light, w: time in seconds
    vec4 light_color; // rgb: directional light tint, w: ambient floor
} pc;

layout(location = 0) out vec4 f_color;

void main() {
    vec4 tex = texture(block_textures, vec3(v_uv, float(v_layer)));
    // Alpha-test for cutout foliage / sprites in the opaque pass.
    if (tex.a < 0.1) {
        discard;
    }
    // Biome tint for the faces a block model marked `tintindex`; white
    // elsewhere, so this is the identity for a texture that carries its own
    // colour.
    vec3 albedo = tex.rgb * v_tint.rgb;
    // Baked face shade (v_ao) plus a time-of-day directional term: ambient floor
    // lifted by the sun/moon contribution, then tinted by the light color.
    float ambient = pc.light_color.w;
    float ndl = max(dot(normalize(v_normal), normalize(pc.sun_dir.xyz)), 0.0);
    float diffuse = ambient + (1.0 - ambient) * ndl;
    f_color = vec4(albedo * v_ao * diffuse * pc.light_color.rgb, tex.a);
}
