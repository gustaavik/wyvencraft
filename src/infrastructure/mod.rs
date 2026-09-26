//! Adapters to the outside world: files, sockets, audio devices, the asset
//! directory. Each implements a port declared further in, or loads data the
//! inner layers consume.

pub mod audio;
pub mod content;
pub mod desktop;
pub mod fs;
pub mod net;
pub mod ops;
pub mod paths;
pub mod profile;
pub mod save;
