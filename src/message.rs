use rg3d::core::algebra::Translation3;
use serde::{Deserialize, Serialize};

// TODO: Create separate PacketMessage that can hold these
#[derive(Debug, Serialize, Deserialize)]
pub enum Message {
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
    LookAround {
        index: u32,
        yaw_delta: f32,
        pitch_delta: f32,
    },
    // Used for synchronizing client
    // UpdatePosition {
    //     index: u32,
    //     position: Translation3<f32>,
    // },
    HeartBeat,
}
