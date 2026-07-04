#version 450

// Matches render::vertex::ChunkVertex.
layout(location = 0) in vec3 position;
layout(location = 1) in vec3 normal;
layout(location = 2) in vec2 uv;
layout(location = 3) in float ao;
layout(location = 4) in uint flags;

// Shared with voxel.frag — layout must stay identical across both stages.
layout(push_constant) uniform PushConstants {
    mat4 view_proj;
    vec4 sun_dir;     // xyz: direction toward the light, w: time in seconds
    vec4 light_color; // rgb: directional light tint, w: ambient floor
} pc;

layout(location = 0) out vec2 v_uv;
layout(location = 1) out float v_ao;
layout(location = 2) out vec3 v_normal;
layout(location = 3) flat out uint v_flags;

void main() {
    gl_Position = pc.view_proj * vec4(position, 1.0);
    v_uv = uv;
    v_ao = ao;
    v_normal = normal;
    v_flags = flags;
}
