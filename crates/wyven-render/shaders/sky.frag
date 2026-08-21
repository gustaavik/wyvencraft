#version 450

// Procedural sky: reconstructs the world-space view ray for each pixel and paints
// a horizon->zenith gradient, a sun disc, a moon, and stars that fade in at night.
// All colors/intensities come from the day/night cycle via push constants.

layout(location = 0) in vec2 v_ndc;

layout(push_constant) uniform PushConstants {
    mat4 inv_view_proj;  // inverse of (proj * view-rotation-only)
    vec4 sun_dir;        // xyz: direction toward the sun, w: star_intensity
    vec4 zenith_color;   // rgb: sky straight up, w: unused
    vec4 horizon_color;  // rgb: sky at horizon, w: moon_intensity
    vec4 sun_color;      // rgb: sun disc tint, w: unused
} pc;

layout(location = 0) out vec4 f_color;

// Cheap 3D hash in [0,1) for procedural star placement.
float hash(vec3 p) {
    p = fract(p * 0.3183099 + vec3(0.1, 0.2, 0.3));
    p *= 17.0;
    return fract(p.x * p.y * p.z * (p.x + p.y + p.z));
}

void main() {
    // World-space view ray: unproject the far-plane point at this pixel. The
    // matrix has no translation, so the result is a pure direction.
    vec4 world = pc.inv_view_proj * vec4(v_ndc, 1.0, 1.0);
    vec3 ray = normalize(world.xyz / world.w);

    vec3 sun_dir = normalize(pc.sun_dir.xyz);
    float star_intensity = pc.sun_dir.w;
    float moon_intensity = pc.horizon_color.w;

    // Vertical gradient (a little color carries below the horizon too).
    float t = smoothstep(-0.1, 0.5, ray.y);
    vec3 color = mix(pc.horizon_color.rgb, pc.zenith_color.rgb, t);

    // Stars in the upper hemisphere, brightening toward the zenith and at night.
    if (star_intensity > 0.001 && ray.y > 0.0) {
        float h = hash(floor(ray * 200.0));
        float star = smoothstep(0.985, 1.0, h);
        color += vec3(star) * star_intensity * smoothstep(0.0, 0.35, ray.y);
    }

    // Moon: soft disc opposite the sun.
    float moon_d = dot(ray, -sun_dir);
    float moon_core = smoothstep(0.9955, 0.9975, moon_d);
    float moon_glow = smoothstep(0.95, 1.0, moon_d) * 0.2;
    color += (vec3(0.86, 0.88, 0.96) * moon_core + vec3(0.55, 0.60, 0.78) * moon_glow)
             * moon_intensity;

    // Sun: bright core + warm bloom, fading out as it dips below the horizon.
    float sun_d = max(dot(ray, sun_dir), 0.0);
    float sun_core = smoothstep(0.9975, 0.9990, sun_d);
    float sun_bloom = pow(sun_d, 256.0) * 0.6 + pow(sun_d, 32.0) * 0.15;
    float sun_visible = smoothstep(-0.15, 0.05, sun_dir.y);
    color += pc.sun_color.rgb * (sun_core + sun_bloom) * sun_visible;

    f_color = vec4(color, 1.0);
}
