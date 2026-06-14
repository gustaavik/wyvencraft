//! A simple box-part humanoid model (head, body, arms, legs) used to render the
//! player in third person and remote players in multiplayer.
//!
//! Dimensions follow the classic Minecraft proportions (in pixels / 16 = blocks).

use glam::Vec3;

/// One rectangular box part of a model, in model-local space (origin at feet).
#[derive(Debug, Clone, Copy)]
pub struct ModelBox {
    /// Centre offset from the model origin.
    pub center: Vec3,
    /// Full extents of the box.
    pub size: Vec3,
}

/// Static layout of the humanoid model parts.
pub struct HumanoidModel {
    pub head: ModelBox,
    pub body: ModelBox,
    pub left_arm: ModelBox,
    pub right_arm: ModelBox,
    pub left_leg: ModelBox,
    pub right_leg: ModelBox,
}

impl HumanoidModel {
    /// Standard player proportions (1 block = 16px).
    pub fn player() -> Self {
        let px = 1.0 / 16.0;
        let leg = Vec3::new(4.0, 12.0, 4.0) * px;
        let arm = Vec3::new(4.0, 12.0, 4.0) * px;
        let body = Vec3::new(8.0, 12.0, 4.0) * px;
        let head = Vec3::splat(8.0) * px;

        Self {
            // Heights stack: legs (0..12px), body (12..24px), head (24..32px).
            right_leg: ModelBox {
                center: Vec3::new(-2.0 * px, 6.0 * px, 0.0),
                size: leg,
            },
            left_leg: ModelBox {
                center: Vec3::new(2.0 * px, 6.0 * px, 0.0),
                size: leg,
            },
            body: ModelBox {
                center: Vec3::new(0.0, 18.0 * px, 0.0),
                size: body,
            },
            right_arm: ModelBox {
                center: Vec3::new(-6.0 * px, 18.0 * px, 0.0),
                size: arm,
            },
            left_arm: ModelBox {
                center: Vec3::new(6.0 * px, 18.0 * px, 0.0),
                size: arm,
            },
            head: ModelBox {
                center: Vec3::new(0.0, 28.0 * px, 0.0),
                size: head,
            },
        }
    }

    pub fn parts(&self) -> [ModelBox; 6] {
        [
            self.head,
            self.body,
            self.left_arm,
            self.right_arm,
            self.left_leg,
            self.right_leg,
        ]
    }
}
