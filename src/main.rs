pub mod level;
mod message;
mod player;

use crate::{level::Level, message::Message, player::Player};
use bincode::{deserialize, serialize};
use laminar::{ErrorKind, Packet, Socket, SocketEvent};
use rg3d::{
    core::{
        algebra::{Isometry3, Translation3, UnitQuaternion, Vector3},
        color::Color,
        color_gradient::{ColorGradient, GradientPoint},
        math::ray::Ray,
        numeric_range::NumericRange,
        pool::{Handle, Pool},
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
use std::{fmt, net::SocketAddr, path::Path, sync::{
        mpsc::{self, Receiver, Sender},
        Arc, RwLock,
    }, thread, time::{self, Instant}};

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
    let (action_sender, receiver) = mpsc::channel();

    // Load level
    let mut level =
        rg3d::futures::executor::block_on(Level::new(&mut engine, "block_scene", receiver));

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

    // Nee
    if SERVER {
        socket = Socket::bind("0.0.0.0:12351").unwrap();
    } else {
        socket = Socket::bind("0.0.0.0:12352").unwrap();
    }

    let (packet_sender, packet_receiver) =
        (socket.get_packet_sender(), socket.get_event_receiver());

    // let _thread = thread::spawn(move || socket.start_polling());

    // Run the event loop of the main window. which will respond to OS and window events and update
    // engine's state accordingly. Engine lets you to decide which event should be handled,
    // this is minimal working example if how it should be.
    let clock = time::Instant::now();
    let mut elapsed_time = 0.0;
    let mut focused = true;

    let mut connected_client = String::new();

    event_loop.run(move |event, _, control_flow| {
        socket.manual_poll(clock);

        // Keeping code here since there is type issue with packet sender/recievers
        while let Ok(event) = packet_receiver.try_recv() {
            match event {
                SocketEvent::Packet(packet) => {
                    if let Ok(message) = deserialize::<Message>(packet.payload()) {
                        match message {
                            Message::HeartBeat => {}
                            _ => {
                                action_sender.send(message).unwrap();
                            }
                        }

                        // Relay any messages to clients
                        if SERVER {
                            let sent_from_server = packet.addr().to_string() == SERVER_ADDRESS;

                            // TODO: send to all connected clients somehow

                            // TODO: only send messages of certain type back to sender, many actions will have already happened locally for the client

                            if sent_from_server {
                                if let Ok(address) = connected_client.parse::<SocketAddr>() {
                                    socket
                                        .send(Packet::unreliable_sequenced(
                                            address,
                                            packet.payload().to_vec(),
                                            None,
                                        ))
                                        .unwrap();
                                }
                            } else {
                                socket
                                    .send(Packet::unreliable_sequenced(
                                        packet.addr(),
                                        packet.payload().to_vec(),
                                        None,
                                    ))
                                    .unwrap();
                            }
                        }
                    }
                }
                SocketEvent::Connect(address) => {
                    let address = address.to_string();
                    if address != SERVER_ADDRESS {
                        connected_client = address;
                    }
                }
                event => println!("unhandled socket event: {:?}", event),
            }
        }

        if !HEADLESS && focused {
            match &event {
                Event::WindowEvent { event, .. } => match event {
                    WindowEvent::KeyboardInput { input, .. } => {
                        if let Some(key_code) = input.virtual_keycode {
                            match key_code {
                                VirtualKeyCode::W => {
                                    let message = Message::MoveForward {
                                        index: player_index,
                                        active: input.state == ElementState::Pressed,
                                    };

                                    // TODO: Send locally so you don't wait for server to move

                                    socket
                                        .send(Packet::unreliable_sequenced(
                                            SERVER_ADDRESS.parse().unwrap(),
                                            serialize(&message).unwrap(),
                                            None,
                                        ))
                                        .unwrap();
                                }
                                VirtualKeyCode::S => {
                                    let message = Message::MoveBackward {
                                        index: player_index,
                                        active: input.state == ElementState::Pressed,
                                    };

                                    socket
                                        .send(Packet::unreliable_sequenced(
                                            SERVER_ADDRESS.parse().unwrap(),
                                            serialize(&message).unwrap(),
                                            None,
                                        ))
                                        .unwrap();
                                }
                                VirtualKeyCode::A => {
                                    let message = Message::MoveLeft {
                                        index: player_index,
                                        active: input.state == ElementState::Pressed,
                                    };

                                    socket
                                        .send(Packet::unreliable_sequenced(
                                            SERVER_ADDRESS.parse().unwrap(),
                                            serialize(&message).unwrap(),
                                            None,
                                        ))
                                        .unwrap();
                                }
                                VirtualKeyCode::D => {
                                    let message = Message::MoveRight {
                                        index: player_index,
                                        active: input.state == ElementState::Pressed,
                                    };

                                    socket
                                        .send(Packet::unreliable_sequenced(
                                            SERVER_ADDRESS.parse().unwrap(),
                                            serialize(&message).unwrap(),
                                            None,
                                        ))
                                        .unwrap();
                                }
                                VirtualKeyCode::Space => {
                                    let message = Message::MoveUp {
                                        index: player_index,
                                        active: input.state == ElementState::Pressed,
                                    };

                                    socket
                                        .send(Packet::unreliable_sequenced(
                                            SERVER_ADDRESS.parse().unwrap(),
                                            serialize(&message).unwrap(),
                                            None,
                                        ))
                                        .unwrap();
                                }
                                _ => (),
                            }
                        }
                    }
                    &WindowEvent::MouseInput { button, state, .. } => {
                        if button == MouseButton::Left {
                            let message = Message::ShootWeapon {
                                index: player_index,
                                active: state == ElementState::Pressed,
                            };

                            socket
                                .send(Packet::unreliable_sequenced(
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

                        let message = Message::LookAround {
                            index: player_index,
                            yaw_delta: mouse_sens * delta.0 as f32,
                            pitch_delta: mouse_sens * delta.1 as f32,
                        };

                        socket
                            .send(Packet::unreliable_sequenced(
                                SERVER_ADDRESS.parse().unwrap(),
                                serialize(&message).unwrap(),
                                None,
                            ))
                            .unwrap();
                    }
                }
                _ => (),
            }
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
                    level.update(&mut engine, TIMESTEP);

                    // Update engine each frame.
                    engine.update(TIMESTEP);

                    socket
                        .send(Packet::unreliable_sequenced(
                            SERVER_ADDRESS.parse().unwrap(),
                            serialize(&Message::HeartBeat).unwrap(),
                            None,
                        ))
                        .unwrap();
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
                _ => (),
            },
            _ => *control_flow = ControlFlow::Poll,
        }
    });
}

fn process_input_event(event: &Event<()>, sender: Sender<Message>) {
    let player_index = 0;

    match event {
        Event::WindowEvent { event, .. } => match event {
            WindowEvent::KeyboardInput { input, .. } => {
                if let Some(key_code) = input.virtual_keycode {
                    match key_code {
                        VirtualKeyCode::W => {
                            sender
                                .send(Message::MoveForward {
                                    index: player_index,
                                    active: input.state == ElementState::Pressed,
                                })
                                .unwrap();
                        }
                        VirtualKeyCode::S => {
                            sender
                                .send(Message::MoveBackward {
                                    index: player_index,
                                    active: input.state == ElementState::Pressed,
                                })
                                .unwrap();
                        }
                        VirtualKeyCode::A => {
                            sender
                                .send(Message::MoveLeft {
                                    index: player_index,
                                    active: input.state == ElementState::Pressed,
                                })
                                .unwrap();
                        }
                        VirtualKeyCode::D => {
                            sender
                                .send(Message::MoveRight {
                                    index: player_index,
                                    active: input.state == ElementState::Pressed,
                                })
                                .unwrap();
                        }
                        VirtualKeyCode::Space => {
                            sender
                                .send(Message::MoveUp {
                                    index: player_index,
                                    active: input.state == ElementState::Pressed,
                                })
                                .unwrap();
                        }
                        _ => (),
                    }
                }
            }
            &WindowEvent::MouseInput { button, state, .. } => {
                if button == MouseButton::Left {
                    sender
                        .send(Message::ShootWeapon {
                            index: player_index,
                            active: state == ElementState::Pressed,
                        })
                        .unwrap();
                }
            }
            _ => {}
        },
        Event::DeviceEvent { event, .. } => {
            if let DeviceEvent::MouseMotion { delta } = event {
                let mouse_sens = 0.5;

                sender
                    .send(Message::LookAround {
                        index: player_index,
                        yaw_delta: mouse_sens * delta.0 as f32,
                        pitch_delta: mouse_sens * delta.1 as f32,
                    })
                    .unwrap();
            }
        }
        _ => (),
    }
}
