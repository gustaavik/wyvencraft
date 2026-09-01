#version 450

// Blocks authored in Blockbench sample render::block_textures — one 256x256
// array layer per texture, chosen per vertex — instead of the shared 16px
// atlas voxel.frag reads. Same vertex shader, same push constants; only the
// texture binding, the tint and the animated layer step differ.

layout(location = 0) in vec2 v_uv;
layout(location = 1) in float v_ao;
layout(location = 2) in vec3 v_normal;
layout(location = 3) flat in uint v_flags;
layout(location = 4) flat in uint v_layer;
layout(location = 5) in vec4 v_tint;
layout(location = 6) flat in uint v_overlay_layer;
layout(location = 7) in vec4 v_overlay_tint;

layout(set = 0, binding = 0) uniform sampler2DArray block_textures;

// Shared with voxel.vert — layout must stay identical across both stages.
layout(push_constant) uniform PushConstants {
    mat4 view_proj;
    vec4 sun_dir;     // xyz: direction toward the light, w: time in seconds
    vec4 light_color; // rgb: directional light tint, w: ambient floor
} pc;

layout(location = 0) out vec4 f_color;

// Must match render::vertex::{ANIM_FRAMES_SHIFT, ANIM_FPS_SHIFT, ANIM_FIELD_MASK}.
const uint ANIM_FRAMES_SHIFT = 8u;
const uint ANIM_FPS_SHIFT = 16u;
const uint ANIM_FIELD_MASK = 0xffu;

// Must match render::vertex::NO_OVERLAY.
const uint NO_OVERLAY = 0xffffffffu;

void main() {
    // An animated texture occupies `frames` consecutive layers, so stepping the
    // layer index is the array's answer to the atlas's UV step. Time comes in as
    // pc.sun_dir.w and wraps at 3600 s, which stays phase-continuous as long as
    // fps * 3600 is a multiple of the frame count.
    uint layer = v_layer;
    uint frames = (v_flags >> ANIM_FRAMES_SHIFT) & ANIM_FIELD_MASK;
    if (frames > 1u) {
        float fps = float((v_flags >> ANIM_FPS_SHIFT) & ANIM_FIELD_MASK);
        layer += uint(mod(floor(pc.sun_dir.w * fps), float(frames)));
    }
    vec4 tex = texture(block_textures, vec3(v_uv, float(layer)));
    // Alpha-test for cutout foliage / sprites in the opaque pass.
    if (tex.a < 0.1) {
        discard;
    }
    // Biome tint for the faces a block model marked `tintindex`; white
    // elsewhere, so this is the identity for a texture that carries its own
    // colour.
    vec3 albedo = tex.rgb * v_tint.rgb;
    // A second layer painted over the first, the way the grass block's tinted
    // side sits on its dirt side. Authored as two coincident quads, merged onto
    // one by the mesher, and blended here rather than depth-tested: coplanar
    // geometry has no dependable draw order. The alpha test above deliberately
    // does not apply to it — an overlay fades against what it covers rather
    // than cutting out against the background.
    if (v_overlay_layer != NO_OVERLAY) {
        vec4 over = texture(block_textures, vec3(v_uv, float(v_overlay_layer)));
        albedo = mix(albedo, over.rgb * v_overlay_tint.rgb, over.a);
    }
    // Baked face shade (v_ao) plus a time-of-day directional term: ambient floor
    // lifted by the sun/moon contribution, then tinted by the light color.
    float ambient = pc.light_color.w;
    float ndl = max(dot(normalize(v_normal), normalize(pc.sun_dir.xyz)), 0.0);
    float diffuse = ambient + (1.0 - ambient) * ndl;
    f_color = vec4(albedo * v_ao * diffuse * pc.light_color.rgb, tex.a);
}
