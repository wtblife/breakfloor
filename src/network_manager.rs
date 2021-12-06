use bincode::{deserialize, serialize, DefaultOptions, Options};
use crossbeam_channel::{Receiver, Sender};
use laminar::{Config, ErrorKind, Packet, Socket, SocketEvent, VirtualConnection};
use serde::{Deserialize, Serialize};
use std::{
    convert::TryInto,
    net::{SocketAddr, ToSocketAddrs},
    thread,
    time::Duration,
};

use crate::{
    game::{Game, GameEvent},
    level::LevelState,
    player::Player,
    player_event::{PlayerEvent, SerializablePlayerState, SerializableVector},
    GameEngine,
};

const SERVER_ADDRESS: &str = "wtblife.ddns.net:12351";

pub struct NetworkManager {
    server_addr: SocketAddr,
    net_sender: Sender<Packet>,
    net_receiver: Receiver<SocketEvent>,
    connections: Vec<PlayerConnection>,
    highest_player_index: u32,
    pub player_index: Option<u32>, // TODO: Should this be in game module or here? It is here because it's easier
}

impl NetworkManager {
    pub fn new() -> Self {
        let server_addr = SERVER_ADDRESS
            .to_socket_addrs()
            .expect("Failed to resolve server hostname")
            .next()
            .expect("Failed to resolve server hostname");

        let config = Config {
            heartbeat_interval: Some(Duration::from_millis(500)),
            ..Default::default()
        };

        let mut socket;

        #[cfg(feature = "server")]
        {
            socket = Socket::bind_with_config("0.0.0.0:12351", config).unwrap();
        }
        #[cfg(not(feature = "server"))]
        {
            socket = Socket::bind_with_config("0.0.0.0:12352", config).unwrap();
        }

        let (sender, receiver) = (socket.get_packet_sender(), socket.get_event_receiver());

        thread::spawn(move || socket.start_polling_with_duration(None));

        #[cfg(not(feature = "server"))]
        {
            sender
                .send(Packet::reliable_ordered(
                    server_addr,
                    serialize(&NetworkMessage::Connected).unwrap(),
                    None,
                ))
                .unwrap();
        }

        Self {
            server_addr,
            net_sender: sender,
            net_receiver: receiver,
            connections: Vec::new(),
            highest_player_index: 0,
            player_index: None,
        }
    }

    pub fn handle_events(&mut self, engine: &mut GameEngine, game: &mut Game) {
        while let Ok(event) = self.net_receiver.try_recv() {
            match event {
                // TODO: Maybe have this call handle_server_events and handle_client_events to make code easier to follow
                SocketEvent::Packet(packet) => {
                    let bincode = DefaultOptions::new()
                        .with_fixint_encoding()
                        .allow_trailing_bytes()
                        .with_limit(1024);

                    if let Ok(message) =
                        &mut bincode.deserialize::<NetworkMessage>(packet.payload())
                    {
                        match message {
                            NetworkMessage::PlayerEvent { index, event } => {
                                if let Some(level) = &mut game.level {
                                    match event {
                                        PlayerEvent::ShootWeapon {
                                            index,
                                            active,
                                            yaw,
                                            pitch,
                                        } => {
                                            #[cfg(feature = "server")]
                                            // Use index from connection on server. Must be set on outer index and inner event
                                            if let Some(net_index) =
                                                self.get_index_for_address(packet.addr())
                                            {
                                                *index = net_index;

                                                if let Some(player) =
                                                    level.get_player_by_index(net_index)
                                                {
                                                    // Validate shoot command
                                                    if !*active || player.can_shoot() {
                                                        level.queue_event(*event);
                                                        self.send_to_all_reliably(message);
                                                    }
                                                }
                                            }

                                            #[cfg(not(feature = "server"))]
                                            level.queue_event(*event);
                                        }
                                        #[cfg(not(feature = "server"))]
                                        PlayerEvent::DestroyBlock { index } => {
                                            level.queue_event(*event);
                                        }
                                        #[cfg(not(feature = "server"))]
                                        PlayerEvent::UpdateState {
                                            timestamp,
                                            index,
                                            position,
                                            velocity,
                                            yaw,
                                            pitch,
                                            shoot,
                                            fuel,
                                        } => {
                                            level.queue_event(*event);
                                        }
                                        // Handles all client predicted events (move events, etc) and player spawn. TODO: Player spawn should be reliable
                                        PlayerEvent::LookAround { index, .. }
                                        | PlayerEvent::MoveBackward { index, .. }
                                        | PlayerEvent::MoveForward { index, .. }
                                        | PlayerEvent::MoveLeft { index, .. }
                                        | PlayerEvent::MoveRight { index, .. } => {
                                            // If event isn't for active player then it hasn't been applied yet. This includes server.
                                            // TODO: This check probably isn't necessary
                                            // if self
                                            //     .player_index
                                            //     .and_then(|id| {
                                            //         if id == *index {
                                            //             Some(id)
                                            //         } else {
                                            //             None
                                            //         }
                                            //     })
                                            //     .is_none()
                                            // {

                                            // Send to all players except the one it was sent from
                                            #[cfg(feature = "server")]
                                            if let Some(net_index) =
                                                self.get_index_for_address(packet.addr())
                                            {
                                                *index = net_index;
                                                level.queue_event(*event);
                                                self.send_to_all_except_address_unreliably(
                                                    packet.addr(),
                                                    message,
                                                    0,
                                                );
                                            }

                                            #[cfg(not(feature = "server"))]
                                            level.queue_event(*event);
                                        }
                                        PlayerEvent::Jump { index } => {
                                            level.queue_event(*event);
                                        }
                                        PlayerEvent::Fly {
                                            index,
                                            active,
                                            fuel,
                                        } => {
                                            #[cfg(feature = "server")]
                                            if let Some(net_index) =
                                                self.get_index_for_address(packet.addr())
                                            {
                                                if let Some(player) =
                                                    level.get_player_by_index(net_index)
                                                {
                                                    *index = net_index;
                                                    *fuel = player.flight_fuel;

                                                    // Validate fly command
                                                    if !*active || player.has_fuel() {
                                                        level.queue_event(*event);
                                                        self.send_to_all_except_address_unreliably(
                                                            packet.addr(),
                                                            message,
                                                            0,
                                                        );
                                                    }
                                                }
                                            }

                                            #[cfg(not(feature = "server"))]
                                            level.queue_event(*event);
                                        }
                                        #[cfg(not(feature = "server"))]
                                        PlayerEvent::KillPlayer { index } => {
                                            level.queue_event(*event);
                                        }
                                        PlayerEvent::SpawnPlayer {
                                            state,
                                            index,
                                            current_player,
                                        } => {
                                            level.queue_event(*event);
                                        }
                                        _ => (),
                                    }
                                }
                            }
                            NetworkMessage::GameEvent { event } => {
                                match event {
                                    #[cfg(feature = "server")]
                                    GameEvent::Joined => {
                                        // Spawn player and send spawn player messages to all
                                        if let Some(level) = &mut game.level {
                                            if let Some(index) =
                                                self.get_index_for_address(packet.addr())
                                            {
                                                // Send events to spawn existing players for player that joined
                                                for player in level.players().iter() {
                                                    let scene = &mut engine.scenes[level.scene];
                                                    let position = player.get_position(scene);
                                                    let velocity = player.get_velocity(scene);
                                                    let message = NetworkMessage::PlayerEvent {
                                                        index: player.index,
                                                        event: PlayerEvent::SpawnPlayer {
                                                            index: player.index,
                                                            state: SerializablePlayerState {
                                                                position: SerializableVector {
                                                                    x: position.x,
                                                                    y: position.y,
                                                                    z: position.z,
                                                                },
                                                                velocity: SerializableVector {
                                                                    x: velocity.x,
                                                                    y: velocity.y,
                                                                    z: velocity.z,
                                                                },
                                                                yaw: player.get_yaw(),
                                                                pitch: player.get_pitch(),
                                                                shoot: player.controller.shoot,
                                                            },
                                                            current_player: false,
                                                        },
                                                    };

                                                    self.send_to_address_reliably(
                                                        packet.addr(),
                                                        &message,
                                                    );
                                                }

                                                // Send spawn player event to all other players
                                                let position = SerializableVector {
                                                    x: 0.0,
                                                    y: 2.0,
                                                    z: 5.0 * (-1.0f32).powi(index as i32),
                                                };
                                                let event = PlayerEvent::SpawnPlayer {
                                                    index: index,
                                                    state: SerializablePlayerState {
                                                        position: position,
                                                        ..Default::default()
                                                    },
                                                    current_player: false,
                                                };
                                                level.queue_event(event);
                                                self.send_to_all_except_address_reliably(
                                                    packet.addr(),
                                                    &NetworkMessage::PlayerEvent {
                                                        index: index,
                                                        event: event,
                                                    },
                                                );

                                                // Send spawn player event to player (with current player true for setting camera)
                                                let event = PlayerEvent::SpawnPlayer {
                                                    index: index,
                                                    state: SerializablePlayerState {
                                                        position: position,
                                                        ..Default::default()
                                                    },
                                                    current_player: true,
                                                };
                                                self.send_to_address_reliably(
                                                    packet.addr(),
                                                    &NetworkMessage::PlayerEvent {
                                                        index: index,
                                                        event: event,
                                                    },
                                                );

                                                println!("player joined: {}", index);
                                            }
                                        }
                                    }
                                    _ => (),
                                }

                                game.queue_event(event.clone());
                            }
                            #[cfg(feature = "server")]
                            NetworkMessage::Connected => {
                                // Respond to connected (first) packet so client can connect.
                                self.net_sender
                                    .send(Packet::reliable_ordered(
                                        packet.addr(),
                                        packet.payload().to_vec(),
                                        None,
                                    ))
                                    .unwrap();
                            }
                            _ => {}
                        }
                    }
                }
                SocketEvent::Connect(address) => {
                    #[cfg(feature = "server")]
                    if let Some(level) = &mut game.level {
                        // Get the highest player index OR the last player index and add 1
                        self.highest_player_index = *self
                            .connections
                            .iter()
                            .map(|connection| connection.player_index)
                            .max()
                            .get_or_insert(self.highest_player_index)
                            + 1;

                        self.connections.push(PlayerConnection {
                            socket_addr: address,
                            player_index: self.highest_player_index,
                        });

                        let reset_level = level.players().len() < 2;
                        let state = if reset_level {
                            LevelState {
                                destroyed_blocks: Vec::new(),
                            }
                        } else {
                            level.state.clone()
                        };

                        // Send message to load level
                        let event = GameEvent::LoadLevel {
                            level: level.name.clone(),
                            state: state.clone(),
                        };

                        if reset_level {
                            // TODO: Fix issue with event not being cloneable
                            // TODO: Fix issue with not being able to re-borrow game
                            game.event_sender
                                .send(GameEvent::LoadLevel {
                                    level: level.name.clone(),
                                    state: state.clone(),
                                })
                                .unwrap();
                        } else {
                            self.send_to_address_reliably(
                                address,
                                &NetworkMessage::GameEvent { event: event },
                            );
                        }
                    }

                    game.queue_event(GameEvent::Connected);

                    println!("{} connected", address.to_string());
                    println!("currently connected: {:?}", self.connections);
                }
                SocketEvent::Disconnect(address) => {
                    #[cfg(feature = "server")]
                    {
                        if let Some(level) = &mut game.level {
                            if let Some(index) = self.get_index_for_address(address) {
                                let event = PlayerEvent::KillPlayer { index: index };
                                level.remove_player(engine, index);
                                self.send_to_all_except_address_reliably(
                                    address,
                                    &NetworkMessage::PlayerEvent {
                                        index: index,
                                        event: event,
                                    },
                                );
                            }
                        }
                        self.connections
                            .retain(|connection| connection.socket_addr != address);
                    }

                    #[cfg(not(feature = "server"))]
                    game.queue_event(GameEvent::Disconnected);

                    println!("{} disconnected", address.to_string());
                    println!("currently connected: {:?}", self.connections);
                }
                SocketEvent::Timeout(address) => {
                    println!("{} timed out", address.to_string());
                }
            }
        }
    }

    pub fn send_to_all_except_address_reliably(
        &mut self,
        address: SocketAddr,
        message: &NetworkMessage,
    ) {
        // Send to all players except one it was sent from
        for connection in self.connections.iter() {
            if connection.socket_addr != address {
                // TODO: Refactor this to use our send function?
                self.net_sender
                    .send(Packet::reliable_ordered(
                        connection.socket_addr,
                        serialize(message).unwrap(),
                        self.get_connection_stream_id(connection),
                    ))
                    .unwrap();
            }
        }
    }

    fn send_to_all_except_address_unreliably(
        &mut self,
        address: SocketAddr,
        message: &NetworkMessage,
        redundancy: i32,
    ) {
        // Send to all players except one it was sent from
        for connection in self.connections.iter() {
            if connection.socket_addr != address {
                for _ in 0..=redundancy {
                    // TODO: Refactor this to use our function?
                    self.net_sender
                        .send(Packet::unreliable_sequenced(
                            connection.socket_addr,
                            serialize(message).unwrap(),
                            None,
                        ))
                        .unwrap();
                }
            }
        }
    }

    pub fn send_to_address_reliably(&mut self, address: SocketAddr, message: &NetworkMessage) {
        self.net_sender
            .send(Packet::reliable_ordered(
                address,
                serialize(message).unwrap(),
                self.get_address_stream_id(address),
            ))
            .unwrap();
    }

    fn send_to_address_unreliably(
        &mut self,
        address: SocketAddr,
        message: &NetworkMessage,
        redundancy: i32,
    ) {
        for _ in 0..=redundancy {
            self.net_sender
                .send(Packet::unreliable_sequenced(
                    address,
                    serialize(message).unwrap(),
                    None,
                ))
                .unwrap();
        }
    }

    pub fn send_to_all_reliably(&mut self, message: &NetworkMessage) {
        for connection in self.connections.iter() {
            self.net_sender
                .send(Packet::reliable_ordered(
                    connection.socket_addr,
                    serialize(message).unwrap(),
                    self.get_connection_stream_id(connection),
                ))
                .unwrap();
        }
    }

    pub fn send_to_all_unreliably(&mut self, message: &NetworkMessage, redundancy: i32) {
        for connection in self.connections.iter() {
            for _ in 0..=redundancy {
                self.net_sender
                    .send(Packet::unreliable_sequenced(
                        connection.socket_addr,
                        serialize(message).unwrap(),
                        None,
                    ))
                    .unwrap();
            }
        }
    }

    pub fn send_to_server_reliably(&mut self, message: &NetworkMessage) {
        self.net_sender
            .send(Packet::reliable_ordered(
                self.server_addr,
                serialize(message).unwrap(),
                self.get_address_stream_id(self.server_addr),
            ))
            .unwrap();
    }

    pub fn send_to_server_unreliably(&mut self, message: &NetworkMessage, redundancy: i32) {
        for _ in 0..=redundancy {
            self.net_sender
                .send(Packet::unreliable_sequenced(
                    self.server_addr,
                    serialize(message).unwrap(),
                    None,
                ))
                .unwrap();
        }
    }

    // pub fn send_to_player_reliably(&mut self) {}

    // pub fn send_to_player_unreliably(&mut self) {}

    pub fn get_address_for_player(&self, index: u32) -> Option<SocketAddr> {
        self.connections
            .iter()
            .find(|connection| connection.player_index == index)
            .and_then(|connection| Some(connection.socket_addr))
    }

    fn get_index_for_address(&self, address: SocketAddr) -> Option<u32> {
        self.connections
            .iter()
            .find(|connection| connection.socket_addr == address)
            .and_then(|connection| Some(connection.player_index))
    }

    fn get_connection_stream_id(&self, connection: &PlayerConnection) -> Option<u8> {
        Some(connection.player_index.to_le_bytes()[0])
    }

    fn get_address_stream_id(&self, address: SocketAddr) -> Option<u8> {
        if address == self.server_addr {
            return Some(0u8);
        };

        self.get_index_for_address(address)
            .and_then(|player_index| Some(player_index.to_le_bytes()[0]))
    }
}

#[derive(Debug, Serialize, Deserialize)]
pub enum NetworkMessage {
    Connected,
    Disconnected,
    PlayerEvent { index: u32, event: PlayerEvent },
    GameEvent { event: GameEvent },
}
#[derive(Debug)]
struct PlayerConnection {
    socket_addr: SocketAddr,
    player_index: u32,
}
