#version 450

// Fullscreen pass: a single oversized triangle covering the screen, generated
// from gl_VertexIndex (no vertex buffer). Drawn with `draw(3, 1, 0, 0)`.

layout(location = 0) out vec2 v_ndc;

void main() {
    // Index 0,1,2 -> (0,0),(2,0),(0,2) -> clip (-1,-1),(3,-1),(-1,3).
    vec2 uv = vec2((gl_VertexIndex << 1) & 2, gl_VertexIndex & 2);
    v_ndc = uv * 2.0 - 1.0;
    // z = 0.0 keeps the triangle at the far plane — the depth range is
    // reversed (see Camera::projection_matrix), so 0 is far and 1 is near. The
    // depth test is Always and nothing is written, so this only matters if the
    // sky ever starts participating in depth; it should be behind everything
    // when it does.
    gl_Position = vec4(v_ndc, 0.0, 1.0);
}
