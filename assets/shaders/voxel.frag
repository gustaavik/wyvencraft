#version 450

layout(location = 0) in vec2 v_uv;
layout(location = 1) in float v_ao;
layout(location = 2) in vec3 v_normal;

layout(set = 0, binding = 0) uniform sampler2D atlas;

layout(location = 0) out vec4 f_color;

void main() {
    vec4 tex = texture(atlas, v_uv);
    // Alpha-test for cutout foliage / sprites in the opaque pass.
    if (tex.a < 0.1) {
        discard;
    }
    // Baked face shade (v_ao) plus a gentle global directional term.
    vec3 light_dir = normalize(vec3(0.4, 1.0, 0.25));
    float diffuse = 0.65 + 0.35 * max(dot(normalize(v_normal), light_dir), 0.0);
    f_color = vec4(tex.rgb * v_ao * diffuse, tex.a);
}
