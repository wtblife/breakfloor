mod level;
mod message;
mod player;

use crate::{
    level::Level,
    message::{ActionMessage, NetworkMessage},
    player::Player,
};
use bincode::{deserialize, serialize};
use crossbeam_channel::{Receiver, Sender};
use laminar::{Config, ErrorKind, Packet, Socket, SocketEvent};
use player::PlayerState;
use rg3d::{
    core::{
        algebra::{Isometry3, Translation3, UnitQuaternion, Vector3},
        color::Color,
        color_gradient::{ColorGradient, GradientPoint},
        math::ray::Ray,
        numeric_range::NumericRange,
        pool::{Handle, Pool},
        profiler::print,
    },
    engine::{resource_manager::ResourceManager, Engine},
    event::{DeviceEvent, ElementState, Event, MouseButton, VirtualKeyCode, WindowEvent},
    event_loop::{ControlFlow, EventLoop},
    gui::node::StubNode,
    physics::{dynamics::RigidBodyBuilder, geometry::ColliderBuilder},
    renderer::surface::{SurfaceBuilder, SurfaceSharedData},
    resource::texture::TextureWrapMode,
    scene::{
        base::BaseBuilder,
        camera::{CameraBuilder, SkyBox},
        graph::Graph,
        mesh::{MeshBuilder, RenderPath},
        node::Node,
        particle_system::{BaseEmitterBuilder, ParticleSystemBuilder, SphereEmitterBuilder},
        physics::RayCastOptions,
        transform::TransformBuilder,
        ColliderHandle, RigidBodyHandle, Scene,
    },
    window::{Fullscreen, WindowBuilder},
};
use std::{
    fmt,
    net::SocketAddr,
    os::windows::process,
    path::Path,
    sync::{
        mpsc::{self},
        Arc, RwLock,
    },
    thread,
    time::{self, Duration, Instant},
};

// Create our own engine type aliases. These specializations are needed, because the engine
// provides a way to extend UI with custom nodes and messages.
type GameEngine = Engine<(), StubNode>;

// Our game logic will be updated at 60 Hz rate.
const TIMESTEP: f32 = 1.0 / 60.0;
const SERVER_ADDRESS: &str = "73.147.172.171:12351";

fn main() {
    const SERVER: bool = cfg!(feature = "server");
    const HEADLESS: bool = cfg!(feature = "headless");

    // Configure main window first.
    let window_builder = WindowBuilder::new()
        .with_visible(!HEADLESS)
        .with_title("Breakfloor");
    // .with_fullscreen(Some(Fullscreen::Borderless(None)));

    // Create event loop that will be used to "listen" events from the OS.
    let event_loop = EventLoop::new();

    // Finally create an instance of the engine.
    let mut engine = GameEngine::new(window_builder, &event_loop, true).unwrap();

    engine
        .renderer
        .set_quality_settings(&rg3d::renderer::QualitySettings {
            use_ssao: false,
            ..Default::default()
        })
        .unwrap();

    if !HEADLESS {
        let window = engine.get_window();
        window.set_cursor_visible(false);
        let _ = window.set_cursor_grab(true);
    }

    // Move this to a Game module or something

    let (action_sender, action_receiver) = mpsc::channel();

    // Load level
    let mut level =
        rg3d::futures::executor::block_on(Level::new(&mut engine, "block_scene", action_receiver));

    let player_index: u32 = if SERVER { 1 } else { 0 };

    rg3d::futures::executor::block_on(level.spawn_player(
        &mut engine,
        0,
        Vector3::new(0.0, 1.0, 0.0),
        player_index == 0,
    ));

    rg3d::futures::executor::block_on(level.spawn_player(
        &mut engine,
        1,
        Vector3::new(0.0, 1.0, 1.0),
        player_index == 1,
    ));

    let mut socket;

    let config = Config {
        heartbeat_interval: Some(Duration::from_millis(1000)),
        ..Default::default()
    };

    if SERVER {
        socket = Socket::bind_with_config("0.0.0.0:12351", config).unwrap();
    } else {
        socket = Socket::bind_with_config("0.0.0.0:12352", config).unwrap();
    }

    let packet_sender: Sender<Packet> = socket.get_packet_sender();
    let packet_receiver: Receiver<SocketEvent> = socket.get_event_receiver();

    if !HEADLESS {
        packet_sender
            .send(Packet::reliable_ordered(
                SERVER_ADDRESS.parse().unwrap(),
                serialize(&NetworkMessage::Connected).unwrap(),
                None,
            ))
            .unwrap();
    }

    let _thread = thread::spawn(move || socket.start_polling());

    // Run the event loop of the main window. which will respond to OS and window events and update
    // engine's state accordingly. Engine lets you to decide which event should be handled,
    // this is minimal working example if how it should be.
    let clock = time::Instant::now();
    let mut elapsed_time = 0.0;
    let mut focused = true;
    let mut cursor_in_window = true;

    let mut connected_client = String::new(); // TODO: Handle multiple clients

    event_loop.run(move |event, _, control_flow| {
        process_network_events(
            &mut engine,
            &packet_receiver,
            &packet_sender,
            &action_sender,
            &mut level,
            SERVER,
            HEADLESS,
            &mut connected_client,
            player_index,
        );

        if !HEADLESS && focused && cursor_in_window {
            process_input_event(&event, &action_sender, &packet_sender, player_index);
        }

        match event {
            Event::MainEventsCleared => {
                // This main game loop - it has fixed time step which means that game
                // code will run at fixed speed even if renderer can't give you desired
                // 60 fps.
                let mut dt = clock.elapsed().as_secs_f32() - elapsed_time;
                while dt >= TIMESTEP {
                    dt -= TIMESTEP;
                    elapsed_time += TIMESTEP;

                    // Run our game's logic.
                    level.update(&mut engine, TIMESTEP, &packet_sender, &mut connected_client);

                    // Update engine each frame.
                    engine.update(TIMESTEP);
                }

                // Rendering must be explicitly requested and handled after RedrawRequested event is received.
                engine.get_window().request_redraw();
            }
            Event::RedrawRequested(_) => {
                // Render at max speed - it is not tied to the game code.
                engine.render(TIMESTEP).unwrap();
            }
            Event::WindowEvent { event, .. } => match event {
                WindowEvent::CloseRequested => *control_flow = ControlFlow::Exit,
                WindowEvent::KeyboardInput { input, .. } => {
                    // Exit game by hitting Escape.
                    if let Some(VirtualKeyCode::Escape) = input.virtual_keycode {
                        *control_flow = ControlFlow::Exit
                    }
                }
                WindowEvent::Resized(size) => {
                    // It is very important to handle Resized event from window, because
                    // renderer knows nothing about window size - it must be notified
                    // directly when window size has changed.
                    engine.renderer.set_frame_size(size.into());
                }
                WindowEvent::Focused(focus) => {
                    focused = focus;
                }
                WindowEvent::CursorEntered { device_id } => {
                    cursor_in_window = true;
                }
                WindowEvent::CursorLeft { device_id } => {
                    cursor_in_window = false;
                }
                _ => (),
            },
            _ => *control_flow = ControlFlow::Poll,
        }
    });
}

fn process_network_events(
    engine: &mut GameEngine,
    packet_receiver: &Receiver<SocketEvent>,
    packet_sender: &Sender<Packet>,
    action_sender: &mpsc::Sender<ActionMessage>,
    level: &mut Level,
    server: bool,
    headless: bool,
    connected_client: &mut String,
    player_index: u32,
) {
    while let Ok(event) = packet_receiver.try_recv() {
        match event {
            SocketEvent::Packet(packet) => {
                if let Ok(message) = deserialize::<NetworkMessage>(packet.payload()) {
                    match message {
                        NetworkMessage::Action { index, action } => {
                            let scene = &engine.scenes[level.scene];
                            let player = level.find_player(index);
                            match action {
                                // TODO: Handle each actionmessage to allow client side prediction
                                ActionMessage::UpdateState {
                                    index,
                                    x,
                                    y,
                                    z,
                                    velocity,
                                    yaw,
                                    pitch,
                                } => {
                                    action_sender.send(action).unwrap();
                                }
                                ActionMessage::ShootWeapon { index, active } => {
                                    if server {
                                        if let Ok(client_addr) =
                                            connected_client.parse::<SocketAddr>()
                                        {
                                            // Validate shoot command
                                            if let Some(player) = player {
                                                if !active {
                                                    packet_sender
                                                        .send(Packet::reliable_ordered(
                                                            client_addr,
                                                            packet.payload().to_vec(),
                                                            None,
                                                        ))
                                                        .unwrap();
                                                } else if player.can_shoot() {
                                                    packet_sender
                                                        .send(Packet::reliable_ordered(
                                                            client_addr,
                                                            packet.payload().to_vec(),
                                                            None,
                                                        ))
                                                        .unwrap();
                                                }
                                            }
                                        }
                                    }
                                    action_sender.send(action).unwrap();
                                }
                                ActionMessage::DestroyBlock { index } => {
                                    action_sender.send(action).unwrap();
                                }
                                _ => {
                                    // Pass received actions along to clients with state updates
                                    if let Some(player) = player {
                                        let position = player.get_position(&scene);
                                        // Actions should have already been applied for current player
                                        if headless || player_index != index {
                                            action_sender.send(action).unwrap();
                                        }
                                        if server {
                                            if let Ok(client_addr) =
                                                connected_client.parse::<SocketAddr>()
                                            {
                                                let state_message = NetworkMessage::Action {
                                                    index: player.index,
                                                    action: ActionMessage::UpdateState {
                                                        index: player.index,
                                                        x: position.x,
                                                        y: position.y,
                                                        z: position.z,
                                                        velocity: player
                                                            .get_vertical_velocity(&scene),
                                                        yaw: player.get_yaw(),
                                                        pitch: player.get_pitch(),
                                                    },
                                                };

                                                // Send state update to other clients along with back to sender
                                                // if client_addr.to_string()
                                                //     != packet.addr().to_string()
                                                // {
                                                //     packet_sender
                                                //         .send(Packet::unreliable_sequenced(
                                                //             packet.addr(),
                                                //             serialize(&state_message).unwrap(),
                                                //             None,
                                                //         ))
                                                //         .unwrap();
                                                // }
                                                packet_sender
                                                    .send(Packet::unreliable_sequenced(
                                                        client_addr,
                                                        serialize(&state_message).unwrap(),
                                                        None,
                                                    ))
                                                    .unwrap();

                                                packet_sender
                                                    .send(Packet::unreliable_sequenced(
                                                        client_addr,
                                                        packet.payload().to_vec(),
                                                        None,
                                                    ))
                                                    .unwrap();
                                            }
                                        }
                                    }
                                }
                            }
                        }
                        NetworkMessage::Connected => {
                            packet_sender
                                .send(Packet::reliable_ordered(
                                    packet.addr(),
                                    serialize(&NetworkMessage::Connected).unwrap(),
                                    None,
                                ))
                                .unwrap();
                        }
                        _ => {}
                    }
                }
            }
            SocketEvent::Connect(address) => {
                // Server establishes connection with itself, but it shouldn't be considered a client
                if address.to_string() != SERVER_ADDRESS {
                    connected_client.clear();
                    connected_client.push_str(&address.to_string()); // TODO: Does the library keep track of these connections?
                }

                println!("{} connected", address.to_string());
            }
            SocketEvent::Disconnect(address) => {
                connected_client.clear();

                println!("{} disconnected", address.to_string());
            }
            SocketEvent::Timeout(address) => {
                println!("{} timed out", address.to_string());
            }
        }
    }
}

fn process_input_event(
    event: &Event<()>,
    action_sender: &mpsc::Sender<ActionMessage>,
    packet_sender: &Sender<Packet>,
    player_index: u32,
) {
    let state_message = ActionMessage::UpdatePreviousState {
        // TODO: Is this going to cause problems by grabbing state after other events have gone through?
        index: player_index,
    };
    match event {
        Event::WindowEvent { event, .. } => match event {
            WindowEvent::KeyboardInput { input, .. } => {
                if let Some(key_code) = input.virtual_keycode {
                    match key_code {
                        VirtualKeyCode::W => {
                            let action = ActionMessage::MoveForward {
                                index: player_index,
                                active: input.state == ElementState::Pressed,
                            };
                            let message = NetworkMessage::Action {
                                index: player_index,
                                action: action,
                            };

                            // Should active = false be reliable since it's only sent once?
                            packet_sender
                                .send(Packet::unreliable_sequenced(
                                    SERVER_ADDRESS.parse().unwrap(),
                                    serialize(&message).unwrap(),
                                    None,
                                ))
                                .unwrap();

                            action_sender.send(state_message).unwrap();
                            action_sender.send(action).unwrap();
                        }
                        VirtualKeyCode::S => {
                            let action = ActionMessage::MoveBackward {
                                index: player_index,
                                active: input.state == ElementState::Pressed,
                            };

                            let message = NetworkMessage::Action {
                                index: player_index,
                                action: action,
                            };

                            packet_sender
                                .send(Packet::unreliable_sequenced(
                                    SERVER_ADDRESS.parse().unwrap(),
                                    serialize(&message).unwrap(),
                                    None,
                                ))
                                .unwrap();

                            action_sender.send(state_message).unwrap();
                            action_sender.send(action).unwrap();
                        }
                        VirtualKeyCode::A => {
                            let action = ActionMessage::MoveLeft {
                                index: player_index,
                                active: input.state == ElementState::Pressed,
                            };
                            let message = NetworkMessage::Action {
                                index: player_index,
                                action: action,
                            };

                            packet_sender
                                .send(Packet::unreliable_sequenced(
                                    SERVER_ADDRESS.parse().unwrap(),
                                    serialize(&message).unwrap(),
                                    None,
                                ))
                                .unwrap();

                            action_sender.send(state_message).unwrap();
                            action_sender.send(action).unwrap();
                        }
                        VirtualKeyCode::D => {
                            let action = ActionMessage::MoveRight {
                                index: player_index,
                                active: input.state == ElementState::Pressed,
                            };
                            let message = NetworkMessage::Action {
                                index: player_index,
                                action: action,
                            };

                            packet_sender
                                .send(Packet::unreliable_sequenced(
                                    SERVER_ADDRESS.parse().unwrap(),
                                    serialize(&message).unwrap(),
                                    None,
                                ))
                                .unwrap();

                            action_sender.send(state_message).unwrap();
                            action_sender.send(action).unwrap();
                        }
                        VirtualKeyCode::Space => {
                            let action = ActionMessage::MoveUp {
                                index: player_index,
                                active: input.state == ElementState::Pressed,
                            };
                            let message = NetworkMessage::Action {
                                index: player_index,
                                action: action,
                            };

                            packet_sender
                                .send(Packet::unreliable_sequenced(
                                    SERVER_ADDRESS.parse().unwrap(),
                                    serialize(&message).unwrap(),
                                    None,
                                ))
                                .unwrap();

                            action_sender.send(state_message).unwrap();
                            action_sender.send(action).unwrap();
                        }
                        _ => (),
                    }
                }
            }
            &WindowEvent::MouseInput { button, state, .. } => {
                if button == MouseButton::Left {
                    let message = NetworkMessage::Action {
                        index: player_index,
                        action: ActionMessage::ShootWeapon {
                            index: player_index,
                            active: state == ElementState::Pressed,
                        },
                    };

                    packet_sender
                        .send(Packet::reliable_ordered(
                            SERVER_ADDRESS.parse().unwrap(),
                            serialize(&message).unwrap(),
                            None,
                        ))
                        .unwrap();
                }
            }
            _ => {}
        },
        Event::DeviceEvent { event, .. } => {
            if let DeviceEvent::MouseMotion { delta } = event {
                let mouse_sens = 0.5;

                let action = ActionMessage::LookAround {
                    index: player_index,
                    yaw_delta: mouse_sens * delta.0 as f32,
                    pitch_delta: mouse_sens * delta.1 as f32,
                };

                let message = NetworkMessage::Action {
                    index: player_index,
                    action: action,
                };

                packet_sender
                    .send(Packet::unreliable_sequenced(
                        SERVER_ADDRESS.parse().unwrap(),
                        serialize(&message).unwrap(),
                        None,
                    ))
                    .unwrap();

                action_sender.send(action).unwrap();
            }
        }
        _ => (),
    }
}
