#version 450

// Matches render::vertex::LineVertex.
layout(location = 0) in vec3 position;
layout(location = 1) in vec3 color;

layout(push_constant) uniform PushConstants {
    mat4 view_proj;
} pc;

layout(location = 0) out vec3 v_color;

void main() {
    gl_Position = pc.view_proj * vec4(position, 1.0);
    v_color = color;
}
