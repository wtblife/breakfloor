use rg3d::core::algebra::{Translation3, Vector3};
use serde::{Deserialize, Serialize};

#[derive(Debug, Serialize, Deserialize, Copy, Clone)]
pub enum ActionMessage {
    ShootWeapon {
        index: u32,
        active: bool,
    },
    MoveForward {
        index: u32,
        active: bool,
    },
    MoveBackward {
        index: u32,
        active: bool,
    },
    MoveLeft {
        index: u32,
        active: bool,
    },
    MoveRight {
        index: u32,
        active: bool,
    },
    MoveUp {
        index: u32,
        active: bool,
    },
    Jump {
        index: u32,
        active: bool,
    },
    LookAround {
        index: u32,
        yaw_delta: f32,
        pitch_delta: f32,
    },
    // Used for synchronizing clients
    UpdateState {
        timestamp: f32,
        index: u32,
        x: f32,
        y: f32,
        z: f32,
        velocity: f32,
        yaw: f32,
        pitch: f32,
    },
    UpdatePreviousState {
        index: u32,
    },
    DestroyBlock {
        index: u32,
    },
    KillPlayer {
        index: u32,
    },
}

#[derive(Debug, Serialize, Deserialize)]
pub enum NetworkMessage {
    Connected,
    Action { index: u32, action: ActionMessage },
}
