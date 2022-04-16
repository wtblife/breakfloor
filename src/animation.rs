use fyrox::{
    animation::{
        machine::{Machine, Parameter, PoseNode, State, Transition},
        Animation,
    },
    core::pool::Handle,
    engine::resource_manager::ResourceManager,
    resource::model::{Model, ModelLoadError},
    scene::{node::Node, Scene},
};

use std::sync::Arc;

// Simple helper method to create a state supplied with PlayAnimation node.
fn create_play_animation_state(
    animation_resource: Model,
    name: &str,
    machine: &mut Machine,
    scene: &mut Scene,
    model: Handle<Node>,
) -> (Handle<Animation>, Handle<State>) {
    // Animations retargetting just makes an instance of animation and binds it to
    // given model using names of bones.
    let animation = *animation_resource
        .retarget_animations(model, scene)
        .get(0)
        .unwrap();
    // Create new PlayAnimation node and add it to machine.
    let node = machine.add_node(PoseNode::make_play_animation(animation));
    // Make a state using the node we've made.
    let state = machine.add_state(State::new(name, node));
    (animation, state)
}

#[derive(Copy, Clone, Default)]
pub struct PlayerAnimationMachineInput {
    pub walk_forward: bool,
    pub shoot: bool,
    pub jump: bool,
    pub fly: bool,
    pub on_ground: bool,
}

pub struct PlayerAnimationMachine {
    machine: Machine,
    pub jump_animation: Handle<Animation>,
}

impl PlayerAnimationMachine {
    // Names of parameters that will be used for transition rules in machine.
    const IDLE_TO_WALK_FORWARD: &'static str = "Idle->WalkForward";
    const IDLE_TO_WALK_BACKWARD: &'static str = "Idle->WalkBackward";
    const IDLE_TO_WALK_LEFT: &'static str = "Idle->WalkLeft";
    const IDLE_TO_WALK_RIGHT: &'static str = "Idle->WalkRight";
    const IDLE_TO_JUMP: &'static str = "Idle->Jump";
    const IDLE_TO_SHOOT: &'static str = "Idle->Shoot";

    const WALK_FORWARD_TO_IDLE: &'static str = "WalkForward->Idle";
    const WALK_FORWARD_TO_WALK_BACKWARD: &'static str = "WalkForward->WalkBackward";
    const WALK_FORWARD_TO_WALK_LEFT: &'static str = "WalkForward->WalkLeft";
    const WALK_FORWARD_TO_WALK_RIGHT: &'static str = "WalkForward->WalkRight";
    const WALK_FORWARD_TO_JUMP: &'static str = "WalkForward->Jump";
    const WALK_FORWARD_TO_SHOOT: &'static str = "WalkForward->Shoot";

    const SHOOT_TO_IDLE: &'static str = "Shoot->Idle";
    const SHOOT_TO_WALK_FORWARD: &'static str = "Shoot->WalkForward";
    const SHOOT_TO_WALK_BACKWARD: &'static str = "Shoot->WalkBackward";
    const SHOOT_TO_WALK_LEFT: &'static str = "Shoot->WalkLeft";
    const SHOOT_TO_WALK_RIGHT: &'static str = "Shoot->WalkRight";
    const SHOOT_TO_JUMP: &'static str = "Shoot->Jump";

    const JUMP_TO_IDLE: &'static str = "Jump->Idle";

    // TODO: Jump, handle run and shoot together (blend upper shoot with lower run)
    // TODO: LATER Death, reload

    pub async fn new(
        scene: &mut Scene,
        model: Handle<Node>,
        resource_manager: ResourceManager,
    ) -> Self {
        let mut machine = Machine::new();

        // Load animations in parallel.
        let (walk_resource, idle_resource, shoot_resource, jump_resource) = fyrox::core::futures::join!(
            resource_manager.request_model("data/animations/walk_forward.fbx"),
            resource_manager.request_model("data/animations/idle.fbx"),
            resource_manager.request_model("data/animations/shoot.fbx"),
            resource_manager.request_model("data/animations/jump.fbx"),
        );

        // Now create three states with different animations.
        let (_, idle_state) =
            create_play_animation_state(idle_resource.unwrap(), "Idle", &mut machine, scene, model);

        let (walk_animation, walk_state) =
            create_play_animation_state(walk_resource.unwrap(), "Walk", &mut machine, scene, model);

        let (shoot_animation, shoot_state) = create_play_animation_state(
            shoot_resource.unwrap(),
            "Shoot",
            &mut machine,
            scene,
            model,
        );

        let (jump_animation, jump_state) =
            create_play_animation_state(jump_resource.unwrap(), "Jump", &mut machine, scene, model);

        scene.animations.get_mut(shoot_animation).set_speed(4.0);
        scene.animations.get_mut(walk_animation).set_speed(2.0);
        scene
            .animations
            .get_mut(jump_animation)
            .set_enabled(false)
            .set_loop(false);

        // // Next, define transitions between states.
        machine.add_transition(Transition::new(
            // A name for debugging.
            "Idle->Walk",
            // Source state.
            idle_state,
            // Target state.
            walk_state,
            // Transition time in seconds.
            0.2,
            // A name of transition rule parameter.
            Self::IDLE_TO_WALK_FORWARD,
        ));
        machine.add_transition(Transition::new(
            "Idle->Shoot",
            idle_state,
            shoot_state,
            0.1,
            Self::IDLE_TO_SHOOT,
        ));
        machine.add_transition(Transition::new(
            "Idle->Jump",
            idle_state,
            jump_state,
            0.2,
            Self::IDLE_TO_JUMP,
        ));

        machine.add_transition(Transition::new(
            "Walk->Idle",
            walk_state,
            idle_state,
            0.2,
            Self::WALK_FORWARD_TO_IDLE,
        ));
        machine.add_transition(Transition::new(
            "Walk->Shoot",
            walk_state,
            shoot_state,
            0.1,
            Self::WALK_FORWARD_TO_SHOOT,
        ));
        machine.add_transition(Transition::new(
            "Walk->Jump",
            walk_state,
            jump_state,
            0.2,
            Self::WALK_FORWARD_TO_JUMP,
        ));

        machine.add_transition(Transition::new(
            "Shoot->Idle",
            shoot_state,
            idle_state,
            0.3,
            Self::SHOOT_TO_IDLE,
        ));
        machine.add_transition(Transition::new(
            "Shoot->Walk",
            shoot_state,
            walk_state,
            0.1,
            Self::SHOOT_TO_WALK_FORWARD,
        ));

        machine.add_transition(Transition::new(
            "Jump->Idle",
            jump_state,
            idle_state,
            0.2,
            Self::JUMP_TO_IDLE,
        ));

        // Define entry state.
        machine.set_entry_state(idle_state);

        Self {
            machine,
            jump_animation,
        }
    }

    pub fn update(&mut self, scene: &mut Scene, dt: f32, input: PlayerAnimationMachineInput) {
        self.machine
            .set_parameter(
                Self::IDLE_TO_WALK_FORWARD,
                Parameter::Rule(input.walk_forward && input.on_ground),
            )
            .set_parameter(Self::IDLE_TO_SHOOT, Parameter::Rule(input.shoot))
            .set_parameter(Self::IDLE_TO_JUMP, Parameter::Rule(input.jump))
            .set_parameter(Self::WALK_FORWARD_TO_JUMP, Parameter::Rule(input.jump))
            // Set transition parameters.
            .set_parameter(
                Self::WALK_FORWARD_TO_IDLE,
                Parameter::Rule(!input.walk_forward || input.fly),
            )
            .set_parameter(Self::WALK_FORWARD_TO_SHOOT, Parameter::Rule(input.shoot))
            .set_parameter(
                Self::SHOOT_TO_IDLE,
                Parameter::Rule(!input.shoot && !input.walk_forward),
            )
            .set_parameter(
                Self::SHOOT_TO_WALK_FORWARD,
                Parameter::Rule(!input.shoot && input.walk_forward),
            )
            // TODO: Add fall/fly animation
            .set_parameter(
                Self::JUMP_TO_IDLE,
                Parameter::Rule(
                    (!input.jump && input.on_ground)
                        || scene.animations.get(self.jump_animation).has_ended(),
                ),
            )
            // Update machine and evaluate final pose.
            .evaluate_pose(&scene.animations, dt)
            // Apply the pose to the graph.
            .apply(&mut scene.graph);
    }
}
