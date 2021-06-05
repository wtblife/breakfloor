use rg3d::core::algebra::{Translation3, Vector3};
use serde::{Deserialize, Serialize};

#[derive(Debug, Serialize, Deserialize, Copy, Clone)]
pub enum PlayerEvent {
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
        position: SerializableVector,
        velocity: SerializableVector,
        yaw: f32,
        pitch: f32,
        shoot: bool,
    },
    DestroyBlock {
        index: u32,
    },
    KillPlayer {
        index: u32,
    },
    SpawnPlayer {
        // TODO: First send all the existing player spawn events to only the player that joined, then send everyone the spawned event for the current player index
        state: SerializablePlayerState, // TODO: Should probably just serialize PlayerState
        index: u32,
        current_player: bool,
    },
}

#[derive(Default, Debug, Serialize, Deserialize, Copy, Clone)]
pub struct SerializablePlayerState {
    pub position: SerializableVector,
    pub velocity: SerializableVector,
    pub yaw: f32,
    pub pitch: f32,
    pub shoot: bool,
}

#[derive(Default, Debug, Serialize, Deserialize, Clone, Copy)]
pub struct SerializableVector {
    pub x: f32,
    pub y: f32,
    pub z: f32,
}