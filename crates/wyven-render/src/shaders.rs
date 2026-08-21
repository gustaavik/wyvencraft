//! Compiled shader modules. `vulkano_shaders::shader!` compiles the GLSL in
//! `assets/shaders/` to SPIR-V at build time and generates typed push-constant
//! structs (e.g. [`voxel_vs::PushConstants`]).

pub mod voxel_vs {
    vulkano_shaders::shader! {
        ty: "vertex",
        path: "shaders/voxel.vert",
    }
}

pub mod voxel_fs {
    vulkano_shaders::shader! {
        ty: "fragment",
        path: "shaders/voxel.frag",
    }
}

pub mod voxel_array_fs {
    vulkano_shaders::shader! {
        ty: "fragment",
        path: "shaders/voxel_array.frag",
    }
}

pub mod sky_vs {
    vulkano_shaders::shader! {
        ty: "vertex",
        path: "shaders/sky.vert",
    }
}

pub mod sky_fs {
    vulkano_shaders::shader! {
        ty: "fragment",
        path: "shaders/sky.frag",
    }
}

pub mod line_vs {
    vulkano_shaders::shader! {
        ty: "vertex",
        path: "shaders/line.vert",
    }
}

pub mod line_fs {
    vulkano_shaders::shader! {
        ty: "fragment",
        path: "shaders/line.frag",
    }
}
