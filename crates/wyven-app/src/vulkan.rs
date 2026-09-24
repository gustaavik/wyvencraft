//! Is there a GPU this engine can run on — answered before anything panics.
//!
//! `vulkano_util::VulkanoContext::new` unwraps every step: no loader, no driver
//! and no suitable device all end in a panic whose message no player can act
//! on. [`check`] walks the same steps first and turns each failure into a
//! [`VulkanUnavailable`] the caller can report and exit on.
//!
//! It also owns device *choice*. What a device must be asked for depends on the
//! device: `image_view_format_swizzle` exists only on portability-subset
//! devices (MoltenVK), and requesting it anywhere else fails device creation;
//! `dynamic_rendering` needs its extension enabled below Vulkan 1.3. So the
//! requirements are derived per device ([`Requirements::for_device`]), and the
//! runner hands vulkano-util the same filter and priority used here, which is
//! what makes the device it creates the one this module vetted.

use std::sync::Arc;

use vulkano::device::physical::{PhysicalDevice, PhysicalDeviceType};
use vulkano::device::{DeviceExtensions, DeviceFeatures, QueueFlags};
#[cfg(target_vendor = "apple")]
use vulkano::instance::InstanceCreateFlags;
use vulkano::instance::{Instance, InstanceCreateInfo, InstanceExtensions};
use vulkano::{Validated, Version, VulkanError, VulkanLibrary};

/// Why this machine cannot run the engine.
#[derive(Debug, thiserror::Error)]
pub enum VulkanUnavailable {
    #[error(
        "no Vulkan loader could be found ({0}). Install or update your graphics driver; \
         virtual machines usually have no Vulkan support"
    )]
    NoLoader(String),
    #[error(
        "no graphics driver with Vulkan support was found ({0}). Update your graphics \
         driver; virtual machines usually have no Vulkan support"
    )]
    NoDriver(String),
    #[error("Vulkan found no graphics device ({0}). Update your graphics driver")]
    NoDevice(String),
    #[error(
        "the graphics device {device} lacks {missing}, which this game requires. Update \
         your graphics driver",
        missing = .missing.join(", ")
    )]
    MissingRequirements {
        device: String,
        missing: Vec<&'static str>,
    },
}

/// What a device must support, and be created with.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Requirements {
    pub extensions: DeviceExtensions,
    pub features: DeviceFeatures,
}

impl Requirements {
    /// The requirements for a device with this API version and support for the
    /// portability subset — the two facts they vary by.
    pub fn new(api_version: Version, portability_subset: bool) -> Self {
        let extensions = DeviceExtensions {
            khr_swapchain: true,
            // Core from 1.3; before that the feature is reached through here.
            khr_dynamic_rendering: api_version < Version::V1_3,
            ..DeviceExtensions::empty()
        };
        let features = DeviceFeatures {
            // The world pass uses dynamic rendering (no VkRenderPass).
            dynamic_rendering: true,
            // The block texture array filters anisotropically. A voxel world
            // is mostly ground plane seen edge-on, which is the exact case an
            // isotropic mip chain over-blurs in one axis and aliases in the
            // other — so distant terrain shimmers as the camera turns.
            sampler_anisotropy: true,
            // egui uploads its font/texture images with a component swizzle.
            // Only a portability-subset device (MoltenVK) gates that behind a
            // feature — and only such a device reports the feature at all, so
            // asking for it elsewhere fails device creation outright.
            image_view_format_swizzle: portability_subset,
            ..DeviceFeatures::empty()
        };
        Self {
            extensions,
            features,
        }
    }

    pub fn for_device(device: &PhysicalDevice) -> Self {
        Self::new(
            device.api_version(),
            device.supported_extensions().khr_portability_subset,
        )
    }

    /// Everything this device lacks, by name, for a message a player can read.
    /// Empty means suitable.
    pub fn missing(
        &self,
        extensions: &DeviceExtensions,
        features: &DeviceFeatures,
        has_graphics_queue: bool,
    ) -> Vec<&'static str> {
        let mut missing = Vec::new();
        if !has_graphics_queue {
            missing.push("a graphics queue");
        }
        if self.extensions.khr_swapchain && !extensions.khr_swapchain {
            missing.push("VK_KHR_swapchain");
        }
        if self.extensions.khr_dynamic_rendering && !extensions.khr_dynamic_rendering {
            missing.push("VK_KHR_dynamic_rendering");
        }
        for (wanted, supported, name) in [
            (
                self.features.dynamic_rendering,
                features.dynamic_rendering,
                "dynamic rendering",
            ),
            (
                self.features.sampler_anisotropy,
                features.sampler_anisotropy,
                "anisotropic filtering",
            ),
            (
                self.features.image_view_format_swizzle,
                features.image_view_format_swizzle,
                "image view format swizzle",
            ),
        ] {
            if wanted && !supported {
                missing.push(name);
            }
        }
        missing
    }
}

fn missing_on(device: &PhysicalDevice) -> Vec<&'static str> {
    let has_graphics_queue = device
        .queue_family_properties()
        .iter()
        .any(|q| q.queue_flags.intersects(QueueFlags::GRAPHICS));
    Requirements::for_device(device).missing(
        device.supported_extensions(),
        device.supported_features(),
        has_graphics_queue,
    )
}

/// Whether the engine can run on this device. Handed to vulkano-util as its
/// device filter, so it can never pick one [`check`] would have refused.
pub fn suitable(device: &PhysicalDevice) -> bool {
    missing_on(device).is_empty()
}

/// Lower is preferred: a real GPU over an integrated one over anything
/// emulated. vulkano-util's default order, restated so [`check`] and the
/// runner cannot disagree about which device wins.
pub fn priority(device: &PhysicalDevice) -> u32 {
    match device.properties().device_type {
        PhysicalDeviceType::DiscreteGpu => 1,
        PhysicalDeviceType::IntegratedGpu => 2,
        PhysicalDeviceType::VirtualGpu => 3,
        PhysicalDeviceType::Cpu => 4,
        PhysicalDeviceType::Other => 5,
        _ => 6,
    }
}

/// The instance vulkano-util creates, minus the debug messenger: the surface
/// extensions it appends, and portability enumeration on Apple, without which
/// MoltenVK lists no device at all.
fn instance_create_info(library: &VulkanLibrary) -> InstanceCreateInfo {
    InstanceCreateInfo {
        #[cfg(target_vendor = "apple")]
        flags: InstanceCreateFlags::ENUMERATE_PORTABILITY,
        application_version: Version::V1_3,
        enabled_extensions: library
            .supported_extensions()
            .intersection(&InstanceExtensions {
                khr_surface: true,
                khr_xlib_surface: true,
                khr_xcb_surface: true,
                khr_wayland_surface: true,
                khr_android_surface: true,
                khr_win32_surface: true,
                ext_metal_surface: true,
                ..InstanceExtensions::empty()
            }),
        ..Default::default()
    }
}

/// The device the runner will create, and what to create it with.
#[derive(Debug, Clone)]
pub struct Choice {
    pub device_name: String,
    pub requirements: Requirements,
}

/// Walk loader → driver → device, the steps `VulkanoContext::new` would
/// otherwise panic in, and pick the device it will use.
pub fn check() -> Result<Choice, VulkanUnavailable> {
    let library =
        VulkanLibrary::new().map_err(|err| VulkanUnavailable::NoLoader(err.to_string()))?;
    let create_info = instance_create_info(&library);
    let instance = Instance::new(library, create_info)
        .map_err(|err| VulkanUnavailable::NoDriver(reason(err)))?;
    let devices: Vec<Arc<PhysicalDevice>> = instance
        .enumerate_physical_devices()
        .map_err(|err| VulkanUnavailable::NoDevice(err.to_string()))?
        .collect();

    if let Some(device) = devices
        .iter()
        .filter(|d| suitable(d))
        .min_by_key(|d| priority(d))
    {
        return Ok(Choice {
            device_name: device.properties().device_name.clone(),
            requirements: Requirements::for_device(device),
        });
    }

    // Nothing qualifies: explain using the device a player would expect used.
    let best = devices
        .iter()
        .min_by_key(|d| priority(d))
        .ok_or_else(|| VulkanUnavailable::NoDevice("the driver reported none".into()))?;
    Err(VulkanUnavailable::MissingRequirements {
        device: best.properties().device_name.clone(),
        missing: missing_on(best),
    })
}

/// The driver's own words. `Validated`'s Display for a driver error is only
/// "a non-validation error occurred"; the reason is the `VulkanError` inside.
fn reason(err: Validated<VulkanError>) -> String {
    match err {
        Validated::Error(err) => err.to_string(),
        Validated::ValidationError(err) => err.to_string(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn all_extensions() -> DeviceExtensions {
        DeviceExtensions {
            khr_swapchain: true,
            khr_dynamic_rendering: true,
            ..DeviceExtensions::empty()
        }
    }

    fn all_features() -> DeviceFeatures {
        DeviceFeatures {
            dynamic_rendering: true,
            sampler_anisotropy: true,
            image_view_format_swizzle: true,
            ..DeviceFeatures::empty()
        }
    }

    // The bug this module exists to prevent: a Windows or Linux GPU reports no
    // image_view_format_swizzle (it is not a portability device), and asking
    // for it anyway failed device creation on every one of them.
    #[test]
    fn a_normal_gpu_is_not_asked_for_the_portability_feature() {
        let req = Requirements::new(Version::V1_3, false);
        assert!(!req.features.image_view_format_swizzle);

        let features = DeviceFeatures {
            image_view_format_swizzle: false,
            ..all_features()
        };
        assert!(req.missing(&all_extensions(), &features, true).is_empty());
    }

    #[test]
    fn moltenvk_is_asked_for_the_portability_feature() {
        let req = Requirements::new(Version::V1_2, true);
        assert!(req.features.image_view_format_swizzle);
        let features = DeviceFeatures {
            image_view_format_swizzle: false,
            ..all_features()
        };
        assert_eq!(
            req.missing(&all_extensions(), &features, true),
            vec!["image view format swizzle"]
        );
    }

    #[test]
    fn dynamic_rendering_needs_its_extension_only_before_1_3() {
        assert!(
            !Requirements::new(Version::V1_3, false)
                .extensions
                .khr_dynamic_rendering
        );
        let old = Requirements::new(Version::V1_2, false);
        assert!(old.extensions.khr_dynamic_rendering);

        let extensions = DeviceExtensions {
            khr_swapchain: true,
            ..DeviceExtensions::empty()
        };
        assert_eq!(
            old.missing(&extensions, &all_features(), true),
            vec!["VK_KHR_dynamic_rendering"]
        );
    }

    #[test]
    fn every_gap_is_named() {
        let req = Requirements::new(Version::V1_3, false);
        let missing = req.missing(&DeviceExtensions::empty(), &DeviceFeatures::empty(), false);
        assert_eq!(
            missing,
            vec![
                "a graphics queue",
                "VK_KHR_swapchain",
                "dynamic rendering",
                "anisotropic filtering"
            ]
        );
    }

    #[test]
    fn a_missing_driver_reads_as_advice() {
        let msg = VulkanUnavailable::NoDriver("incompatible driver".into()).to_string();
        assert!(msg.contains("graphics driver"), "{msg}");
        assert!(msg.contains("virtual machines"), "{msg}");
    }
}
