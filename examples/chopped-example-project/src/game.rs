use std::{collections::HashMap, f32::consts::PI};

use chopped_engine::hecs::{Entity, World};
use chopped_engine::kiss3d::{
    color::{BLACK, RED, WHITE},
    glamx::Vec2,
    resource::TextureManager,
    scene::SceneNode2d,
};
use chopped_engine::rapier2d::{
    control::KinematicCharacterController, parry::query::details::ShapeCastOptions, prelude::*,
};
use chopped_engine::uuid::Uuid;

use chopped_engine::{ timestepper::GAME_TIME_DELTA,
    util::GameRectangle, log};

use crate::{
    config::GameConfig,
    editor::{EditMode, EditableObjectType},
    level::{Level, MovingPlatform, SpawnCloud, StaticPlatform},
};

// Collision groups: players collide with obstacles but not with each other
pub const PLAYER_GROUP: Group = Group::GROUP_1;
pub const STATIC_PLATFORM_GROUP: Group = Group::GROUP_2;
pub const SPAWN_CLOUD_GROUP: Group = Group::GROUP_3;
pub const EXIT_GROUP: Group = Group::GROUP_4;
pub const WALL_GROUP: Group = Group::GROUP_5;
pub const MOVING_PLATFORM_GROUP: Group = Group::GROUP_6;
/// The editor handles that mark a moving platform's point 2. They're only ever picked
/// by unfiltered point queries, so nothing is allowed to collide with them.
pub const MOVING_PLATFORM_ENDPOINT_GROUP: Group = Group::GROUP_7;
pub const SLEEPING_BODY_GROUP: Group = Group::GROUP_8;

// add a little bit of buffer so we don't clip the player with the cloud
pub const CLOUD_BUFFER: f32 = 1.0;

/// Gap between the top of the player and the sleeping body riding on them, in pixels.
pub const SLEEPING_BODY_BUFFER: f32 = 1.0;

/// How far away from point 1 a freshly created moving platform puts point 2.
pub const DEFAULT_ENDPOINT_OFFSET: Vec2 = Vec2::new(50., 0.);
/// Patrol speed of a freshly created moving platform, in pixels per second.
pub const DEFAULT_MOVING_PLATFORM_SPEED: f32 = 20.;
pub const EXIT_OUTLINE: f32 = 3.;

// tag
pub struct StaticPlatformTag {}
pub struct SpawnCloudTag {}
pub struct MovingPlatformTag {}

/// The path a moving platform patrols, in game (pixel) space.
#[derive(Debug, Clone, Copy)]
pub struct MovementPath {
    /// The kinematic body for the platform.
    body: RigidBodyHandle,
    // starting point at the start of the level
    point_1: Vec2,
    // point that we actually can edit
    point_2: Vec2,
    /// Pixels per second.
    speed: f32,
    /// How far along the path the platform is: 0.0 sits on point 1, 1.0 on point 2.
    progress: f32,
    /// True while travelling from point 1 towards point 2.
    forward: bool,
    /// The editor handle sitting on point 2, so that point can be picked and dragged
    /// like any other object.
    endpoint_collider: ColliderHandle,
}

pub struct Wall {}

pub struct Exit {}

#[derive(Debug, PartialEq, Clone, Copy)]
pub struct MoveInputState {
    pub thrust: bool,
    pub up: bool,
    pub down: bool,
    pub left: bool,
    pub right: bool,
}
impl Default for MoveInputState {
    fn default() -> Self {
        Self {
            thrust: false,
            up: false,
            down: false,
            left: false,
            right: false,
        }
    }
}

#[derive(Debug, PartialEq)]
pub enum CollisionType {
    Horizontal,
    Vertical,
    Crushed,
    Exit,
    None,
}

#[derive(Debug, PartialEq)]
pub enum GameEvents {
    Respawn,
    Exit,
    None,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct SceneNodeKey(Uuid);

pub struct PlayerNodeKeys {
    center: SceneNodeKey,
    left_thruster: SceneNodeKey,
    right_thruster: SceneNodeKey,
    bottom_thruster: SceneNodeKey,
}

pub struct ExitNodeKeys {
    center: SceneNodeKey,
    outline: SceneNodeKey,
}

pub struct SleepingBody {
    body: RigidBodyHandle,
    collider: ColliderHandle,
    node: SceneNodeKey,
}

pub struct GameLogic {
    config: GameConfig,
    current_level: Level,
    world: World,
    physics: PhysicsWorld,
    player: Entity,
    starting_cloud: Entity,
    current_spawn_cloud: Entity,
    current_player_input: MoveInputState,
    selected_collider: Option<ColliderHandle>,
    exit_entity: Entity,
    cloud_map: HashMap<ColliderHandle, Entity>,
    static_platform_map: HashMap<ColliderHandle, Entity>,
    moving_platform_map: HashMap<ColliderHandle, Entity>,
    /// Maps a moving platform's point 2 handle back to the platform entity that owns it.
    moving_platform_endpoint_map: HashMap<ColliderHandle, Entity>,
    render_map: HashMap<SceneNodeKey, SceneNode2d>,
    edit_mode: EditMode,
}

/// GameLogic should NEVER leak any physics space vectors outside of the physics world.
/// As far as the rest of the codebase is concerned, it's ONLY worried about pixel (game) space.
impl GameLogic {
    pub fn new(
        config: GameConfig,
        level: Level,
        root_scene: &mut SceneNode2d,
        texture_manager: &mut TextureManager,
    ) -> Self {
        // delete all children first
        // note: we store all of the children in a vec first to prevent runtime panics
        // otherwise we would be freeing a child while they're still being borrowed
        let children: Vec<SceneNode2d> = root_scene.data().children().to_owned();
        for mut child in children {
            child.detach();
        }
        // TODO: for now, we hardcode left and right side players and require there to be only one of each
        let mut world = hecs::World::new();
        let mut physics = PhysicsWorld::new();
        // Set the physics timestep to the same timestep as our game
        physics.integration_parameters.dt = GAME_TIME_DELTA;
        let mut render_map = HashMap::new();

        // spawn collision box for the players to be trapped in while bullets are shooting at them
        Self::spawn_walls(
            &mut world,
            &mut physics,
            &config,
            &level,
            root_scene,
            texture_manager,
            &mut render_map,
        );

        // spawn any extra static platforms
        let static_platform_map = Self::spawn_static_platforms(
            &mut world,
            &mut physics,
            &config,
            &level,
            root_scene,
            texture_manager,
            &mut render_map,
        );

        // platforms that move back and forth between two points
        let (moving_platform_map, moving_platform_endpoint_map) = Self::spawn_moving_platforms(
            &mut world,
            &mut physics,
            &config,
            &level,
            root_scene,
            texture_manager,
            &mut render_map,
        );

        // the cloud the player spawns on top of
        let (starting_cloud, cloud_map) = Self::spawn_spawn_clouds(
            &mut world,
            &mut physics,
            &config,
            &level,
            root_scene,
            texture_manager,
            &mut render_map,
        );

        // exit from the level
        let exit_entity = Self::spawn_exit(
            &mut world,
            &mut physics,
            &config,
            &level,
            root_scene,
            texture_manager,
            &mut render_map,
        );

        let starting_player_position =
            Self::get_player_spawn_position(&mut world, &config, &physics, starting_cloud);

        let player = Self::spawn_player(
            &mut world,
            &mut physics,
            &config,
            starting_player_position,
            root_scene,
            texture_manager,
            &mut render_map,
        );

        // The broad-phase spatial index used by scene queries (incl. the character
        // controller's move_shape) isn't populated until a step runs; without this,
        // the very first update_position_with_input call queries a stale/empty index
        // and misses newly-inserted colliders entirely.
        physics.step();

        Self::sync_scene_nodes(
            &config,
            &mut world,
            &physics,
            &mut render_map,
            &MoveInputState::default(),
        );

        Self {
            config,
            current_level: level,
            world,
            physics,
            static_platform_map,
            moving_platform_map,
            moving_platform_endpoint_map,
            cloud_map,
            current_spawn_cloud: starting_cloud,
            player,
            current_player_input: MoveInputState::default(),
            starting_cloud,
            selected_collider: None,
            exit_entity,
            edit_mode: EditMode::None,
            render_map,
        }
    }

    pub fn get_config(&self) -> &GameConfig {
        &self.config
    }

    fn sync_scene_nodes(
        config: &GameConfig,
        world: &mut World,
        physics: &PhysicsWorld,
        render_map: &mut HashMap<SceneNodeKey, SceneNode2d>,
        input: &MoveInputState,
    ) {
        for (collider_handle, render_node_key) in
            world.query_mut::<(&ColliderHandle, &SceneNodeKey)>()
        {
            let rect =
                Self::get_game_rectangle_from_collider_handle(config, physics, collider_handle);
            let node = render_map
                .get_mut(render_node_key)
                .expect("this should exist");
            node.set_position(rect.get_position());
            node.set_local_scale(rect.width, rect.height);
        }
        // render player
        for (collider_handle, player_node_keys) in
            world.query_mut::<(&ColliderHandle, &PlayerNodeKeys)>()
        {
            let center_rect =
                Self::get_game_rectangle_from_collider_handle(config, physics, collider_handle);
            let node = render_map
                .get_mut(&player_node_keys.center)
                .expect("this should exist");
            node.set_position(center_rect.get_position());
            node.set_local_scale(center_rect.width, center_rect.height);

            let left_thruster = render_map
                .get_mut(&player_node_keys.left_thruster)
                .expect("this should exist");
            left_thruster
                .set_position(center_rect.get_position() + Vec2::new(center_rect.height, 0.));
            left_thruster.set_local_scale(center_rect.width, center_rect.height);
            left_thruster.set_rotation(PI / 2.);
            left_thruster.set_visible(input.left);

            let right_thruster = render_map
                .get_mut(&player_node_keys.right_thruster)
                .expect("this should exist");
            right_thruster
                .set_position(center_rect.get_position() + Vec2::new(-center_rect.height, 0.));
            right_thruster.set_local_scale(center_rect.width, center_rect.height);
            right_thruster.set_rotation(-PI / 2.);
            right_thruster.set_visible(input.right);

            let bottom_thruster = render_map
                .get_mut(&player_node_keys.bottom_thruster)
                .expect("this should exist");
            bottom_thruster
                .set_position(center_rect.get_position() + Vec2::new(0., -center_rect.height));
            bottom_thruster.set_local_scale(center_rect.width, center_rect.height);
            bottom_thruster.set_visible(input.thrust);
        }
        // render exit
        for (collider_handle, exit_node_keys) in
            world.query_mut::<(&ColliderHandle, &ExitNodeKeys)>()
        {
            let center_rect =
                Self::get_game_rectangle_from_collider_handle(config, physics, collider_handle);
            let node = render_map
                .get_mut(&exit_node_keys.center)
                .expect("this should exist");
            node.set_position(center_rect.get_position());
            node.set_local_scale(center_rect.width, center_rect.height);

            let outline_rect = render_map
                .get_mut(&exit_node_keys.outline)
                .expect("this should exist");
            outline_rect.set_position(center_rect.get_position());
            outline_rect.set_local_scale(
                center_rect.width + EXIT_OUTLINE,
                center_rect.height + EXIT_OUTLINE,
            );
        }
        // the sleeping body is its own physics body, so it's drawn wherever it ended up
        for sleeping_body in world.query_mut::<&SleepingBody>() {
            let position = config.convert_vec2_physics_to_pixel(
                physics.colliders[sleeping_body.collider]
                    .position()
                    .translation,
            );
            // the capsule node is already the right size, only its position moves
            render_map
                .get_mut(&sleeping_body.node)
                .expect("this should exist")
                .set_position(position);
        }
    }

    pub fn set_edit_mode(&mut self, edit_mode: EditMode) {
        log!("{:?}", edit_mode);
        self.edit_mode = edit_mode;
        self.park_moving_platforms();
    }

    /// Puts every moving platform back on point 1, restarts its movement path
    fn park_moving_platforms(&mut self) {
        let pixel_to_physics_scale = self.config.pixel_to_physics_scale();
        let bodies = &mut self.physics.bodies;

        for (_, path) in self
            .world
            .query_mut::<(&MovingPlatformTag, &mut MovementPath)>()
        {
            path.progress = 0.;
            path.forward = true;
            // we set the moving platforms movement by position, we don't want to add velocity to the player
            bodies[path.body].set_position(
                Pose2::from_translation(path.point_1 * pixel_to_physics_scale),
                true,
            );
        }
    }

    /// Walks every moving platform one tick along its path, bouncing off each end.
    ///
    /// While editing, the colliders are what the user is dragging around, so the points
    /// are read back out of them instead.
    fn update_moving_platforms(&mut self) {
        let editing = self.edit_mode != EditMode::None;
        let pixel_to_physics_scale = self.config.pixel_to_physics_scale();
        let physics_to_pixel_scale = self.config.physics_to_pixel_scale;
        let bodies = &mut self.physics.bodies;
        let colliders = &mut self.physics.colliders;

        for (_, path, collider_handle) in
            self.world
                .query_mut::<(&MovingPlatformTag, &mut MovementPath, &ColliderHandle)>()
        {
            if editing {
                // the body is what the editor drags, and the collider only catches up to
                // it on the next step, so read point 1 straight off the body
                path.point_1 = bodies[path.body].position().translation * physics_to_pixel_scale;
                path.point_2 = colliders[path.endpoint_collider].position().translation
                    * physics_to_pixel_scale;
                // the handle stands in for the platform, so it has to keep matching its size
                let half_extents = colliders[*collider_handle]
                    .shape()
                    .as_cuboid()
                    .expect("every physics object here should be a cuboid")
                    .half_extents;
                colliders[path.endpoint_collider]
                    .shape_mut()
                    .as_cuboid_mut()
                    .expect("every physics object here should be a cuboid")
                    .half_extents = half_extents;
                continue;
            }

            let travel = path.point_2 - path.point_1;
            let distance = travel.length();
            // a platform whose two points sit on top of each other just stays put
            if distance > f32::EPSILON {
                // progress runs 0..1 over the whole path, so a pixels/second speed has to
                // be divided by the path length to become progress/second
                let step = (path.speed / distance) * GAME_TIME_DELTA;
                path.progress += if path.forward { step } else { -step };
                // bounce off the ends, reflecting the overshoot back into the new direction
                if path.progress >= 1. {
                    path.forward = false;
                } else if path.progress <= 0. {
                    path.forward = true;
                }
                // a platform fast enough to cross the whole path in one tick would
                // otherwise reflect to somewhere off the path entirely
                path.progress = path.progress.clamp(0., 1.);
            }

            // A kinematic move rather than a teleport
            let position = path.point_1 + travel * path.progress;
            bodies[path.body].set_next_kinematic_translation(position * pixel_to_physics_scale);
        }
    }

    /// The kinematic body driving this collider, if the collider belongs to a moving
    /// platform. Anything else in the level is a loose collider with no body.
    fn moving_platform_body(&mut self, collider_handle: ColliderHandle) -> Option<RigidBodyHandle> {
        let platform_entity = *self.moving_platform_map.get(&collider_handle)?;
        Some(
            self.world
                .query_one_mut::<&MovementPath>(platform_entity)
                .expect("a moving platform always has a patrol path")
                .body,
        )
    }

    /// A point 2 handle isn't an object in its own right, so a click on one counts as a
    /// click on the moving platform that owns it. Any other handle passes straight through.
    fn resolve_selection_to_platform(&mut self, collider_handle: ColliderHandle) -> ColliderHandle {
        let Some(platform_entity) = self.moving_platform_endpoint_map.get(&collider_handle) else {
            return collider_handle;
        };
        *self
            .world
            .query_one_mut::<&ColliderHandle>(*platform_entity)
            .expect("a moving platform always has a collider")
    }

    pub fn get_edit_mode(&self) -> EditMode {
        self.edit_mode
    }

    fn spawn_static_physics_object(
        root_scene: &mut SceneNode2d,
        physics: &mut PhysicsWorld,
        config: &GameConfig,
        x: f32,
        y: f32,
        width: f32,
        height: f32,
        membership: Group,
        render_map: &mut HashMap<SceneNodeKey, SceneNode2d>,
    ) -> (ColliderHandle, SceneNodeKey) {
        let pixel_to_physics_scale = config.pixel_to_physics_scale();
        let collider = ColliderBuilder::cuboid(
            width * pixel_to_physics_scale / 2.,
            height * pixel_to_physics_scale / 2.,
        )
        .collision_groups(InteractionGroups::new(
            membership,
            PLAYER_GROUP | SLEEPING_BODY_GROUP,
            InteractionTestMode::And,
        ))
        .position(Pose2 {
            rotation: Rot2::default(),
            translation: Vec2::new(x, y) * pixel_to_physics_scale,
        })
        .build();
        let scene_node_key = SceneNodeKey(Uuid::new_v4());
        // unit rectangle, we will change the size and position during the sync
        let rect = root_scene.add_rectangle(1.0, 1.0);
        render_map.insert(scene_node_key, rect);
        (physics.colliders.insert(collider), scene_node_key)
    }

    fn spawn_kinematic_physics_object(
        root_scene: &mut SceneNode2d,
        physics: &mut PhysicsWorld,
        config: &GameConfig,
        x: f32,
        y: f32,
        width: f32,
        height: f32,
        membership: Group,
        render_map: &mut HashMap<SceneNodeKey, SceneNode2d>,
    ) -> (RigidBodyHandle, ColliderHandle, SceneNodeKey) {
        let pixel_to_physics_scale = config.pixel_to_physics_scale();
        let rigid_body = RigidBodyBuilder::kinematic_position_based()
            .translation(Vec2::new(x, y) * pixel_to_physics_scale)
            // a platform moves forever, so it must never be allowed to doze off
            .can_sleep(false)
            .build();
        // the body carries the world position, so the collider sits at its origin
        let collider = ColliderBuilder::cuboid(
            width * pixel_to_physics_scale / 2.,
            height * pixel_to_physics_scale / 2.,
        )
        .collision_groups(InteractionGroups::new(
            membership,
            PLAYER_GROUP | SLEEPING_BODY_GROUP,
            InteractionTestMode::And,
        ))
        .build();
        let (body_handle, collider_handle) = physics.insert(rigid_body, collider);
        let scene_node_key = SceneNodeKey(Uuid::new_v4());
        // unit rectangle, we will change the size and position during the sync
        let rect = root_scene.add_rectangle(1.0, 1.0);
        render_map.insert(scene_node_key, rect);
        (body_handle, collider_handle, scene_node_key)
    }

    fn spawn_walls(
        world: &mut hecs::World,
        physics: &mut PhysicsWorld,
        config: &GameConfig,
        level: &Level,
        root_scene: &mut SceneNode2d,
        _texture_manager: &mut TextureManager,
        render_map: &mut HashMap<SceneNodeKey, SceneNode2d>,
    ) {
        // left wall
        let (left_wall_handle, left_wall_render_node) = Self::spawn_static_physics_object(
            root_scene,
            physics,
            config,
            -level.player_trapped_box_width / 2.,
            0.,
            level.player_trapped_box_wall_width,
            level.player_trapped_box_height,
            WALL_GROUP,
            render_map,
        );
        world.spawn((Wall {}, left_wall_handle, left_wall_render_node));

        // right wall
        let (right_wall_handle, right_wall_render_node) = Self::spawn_static_physics_object(
            root_scene,
            physics,
            config,
            level.player_trapped_box_width / 2.,
            0.,
            level.player_trapped_box_wall_width,
            level.player_trapped_box_height,
            WALL_GROUP,
            render_map,
        );
        world.spawn((Wall {}, right_wall_handle, right_wall_render_node));

        // up wall
        let (up_wall_handle, up_wall_render_node) = Self::spawn_static_physics_object(
            root_scene,
            physics,
            config,
            0.,
            level.player_trapped_box_height / 2.,
            level.player_trapped_box_width,
            level.player_trapped_box_wall_width,
            WALL_GROUP,
            render_map,
        );
        world.spawn((Wall {}, up_wall_handle, up_wall_render_node));

        // down wall
        let (down_wall_handle, down_wall_render_node) = Self::spawn_static_physics_object(
            root_scene,
            physics,
            config,
            0.,
            -level.player_trapped_box_height / 2.,
            level.player_trapped_box_width,
            level.player_trapped_box_wall_width,
            WALL_GROUP,
            render_map,
        );
        world.spawn((Wall {}, down_wall_handle, down_wall_render_node));
    }

    fn spawn_static_platform(
        world: &mut hecs::World,
        physics: &mut PhysicsWorld,
        config: &GameConfig,
        static_platform: &StaticPlatform,
        root_scene: &mut SceneNode2d,
        _texture_manager: &mut TextureManager,
        render_map: &mut HashMap<SceneNodeKey, SceneNode2d>,
    ) -> (ColliderHandle, Entity) {
        let (collider_handle, render_node) = Self::spawn_static_physics_object(
            root_scene,
            physics,
            config,
            static_platform.position.0,
            static_platform.position.1,
            static_platform.width,
            static_platform.height,
            STATIC_PLATFORM_GROUP,
            render_map,
        );
        let entity = world.spawn((StaticPlatformTag {}, collider_handle, render_node));

        (collider_handle, entity)
    }

    fn spawn_static_platforms(
        world: &mut hecs::World,
        physics: &mut PhysicsWorld,
        config: &GameConfig,
        level: &Level,
        root_scene: &mut SceneNode2d,
        texture_manager: &mut TextureManager,
        render_map: &mut HashMap<SceneNodeKey, SceneNode2d>,
    ) -> HashMap<ColliderHandle, Entity> {
        let mut static_platform_map = HashMap::new();
        for static_platform in &level.static_platforms {
            let (collider_handle, entity) = Self::spawn_static_platform(
                world,
                physics,
                config,
                static_platform,
                root_scene,
                texture_manager,
                render_map,
            );
            static_platform_map.insert(collider_handle, entity);
        }
        static_platform_map
    }

    /// Spawns the platform on point 1 plus the handle that marks point 2. The handle is
    /// a plain rectangle in a group nothing collides with; it only exists so the editor
    /// has something to click on, so it stays hidden outside of edit mode.
    fn spawn_moving_platform(
        world: &mut hecs::World,
        physics: &mut PhysicsWorld,
        config: &GameConfig,
        moving_platform: &MovingPlatform,
        root_scene: &mut SceneNode2d,
        _texture_manager: &mut TextureManager,
        render_map: &mut HashMap<SceneNodeKey, SceneNode2d>,
    ) -> (ColliderHandle, ColliderHandle, Entity) {
        let (body, collider_handle, render_node) = Self::spawn_kinematic_physics_object(
            root_scene,
            physics,
            config,
            moving_platform.point_1.0,
            moving_platform.point_1.1,
            moving_platform.width,
            moving_platform.height,
            MOVING_PLATFORM_GROUP,
            render_map,
        );

        let pixel_to_physics_scale = config.pixel_to_physics_scale();
        let collider = ColliderBuilder::cuboid(
            moving_platform.width * pixel_to_physics_scale / 2.,
            moving_platform.height * pixel_to_physics_scale / 2.,
        )
        .collision_groups(InteractionGroups::new(
            MOVING_PLATFORM_ENDPOINT_GROUP,
            PLAYER_GROUP,
            InteractionTestMode::And,
        ))
        .position(Pose2 {
            rotation: Rot2::default(),
            translation: Vec2::new(moving_platform.point_2.0, moving_platform.point_2.1)
                * pixel_to_physics_scale,
        })
        .build();
        let endpoint_collider = physics.colliders.insert(collider);

        let entity = world.spawn((
            MovingPlatformTag {},
            collider_handle,
            render_node,
            MovementPath {
                body,
                point_1: moving_platform.get_vec2_point_1(),
                point_2: moving_platform.get_vec2_point_2(),
                speed: moving_platform.speed,
                progress: 0.,
                forward: true,
                endpoint_collider,
            },
        ));

        (collider_handle, endpoint_collider, entity)
    }

    fn spawn_moving_platforms(
        world: &mut hecs::World,
        physics: &mut PhysicsWorld,
        config: &GameConfig,
        level: &Level,
        root_scene: &mut SceneNode2d,
        texture_manager: &mut TextureManager,
        render_map: &mut HashMap<SceneNodeKey, SceneNode2d>,
    ) -> (
        HashMap<ColliderHandle, Entity>,
        HashMap<ColliderHandle, Entity>,
    ) {
        let mut moving_platform_map = HashMap::new();
        let mut endpoint_map = HashMap::new();
        for moving_platform in &level.moving_platforms {
            let (collider_handle, endpoint_collider, entity) = Self::spawn_moving_platform(
                world,
                physics,
                config,
                moving_platform,
                root_scene,
                texture_manager,
                render_map,
            );
            moving_platform_map.insert(collider_handle, entity);
            endpoint_map.insert(endpoint_collider, entity);
        }
        (moving_platform_map, endpoint_map)
    }

    fn spawn_spawn_cloud(
        world: &mut hecs::World,
        physics: &mut PhysicsWorld,
        config: &GameConfig,
        spawn_cloud: &SpawnCloud,
        root_scene: &mut SceneNode2d,
        texture_manager: &mut TextureManager,
        render_map: &mut HashMap<SceneNodeKey, SceneNode2d>,
    ) -> (ColliderHandle, Entity) {
        let (collider_handle, render_node) = Self::spawn_static_physics_object(
            root_scene,
            physics,
            config,
            spawn_cloud.position.0,
            spawn_cloud.position.1,
            spawn_cloud.width,
            spawn_cloud.height,
            SPAWN_CLOUD_GROUP,
            render_map,
        );
        render_map
            .get_mut(&render_node)
            .expect("we just inserted it")
            .set_texture(texture_manager.get("cloud").unwrap());
        let entity = world.spawn((SpawnCloudTag {}, collider_handle, render_node));
        (collider_handle, entity)
    }

    fn spawn_spawn_clouds(
        world: &mut hecs::World,
        physics: &mut PhysicsWorld,
        config: &GameConfig,
        level: &Level,
        root_scene: &mut SceneNode2d,
        texture_manager: &mut TextureManager,
        render_map: &mut HashMap<SceneNodeKey, SceneNode2d>,
    ) -> (Entity, HashMap<ColliderHandle, Entity>) {
        let mut starting_cloud: Option<hecs::Entity> = None;
        let mut hash_map = HashMap::new();
        for spawn_cloud in &level.spawn_clouds {
            let (collider_handle, entity) = Self::spawn_spawn_cloud(
                world,
                physics,
                config,
                spawn_cloud,
                root_scene,
                texture_manager,
                render_map,
            );
            if starting_cloud.is_none() {
                starting_cloud = Some(entity);
            }
            hash_map.insert(collider_handle, entity);
        }
        (
            starting_cloud
                .expect("we should have at least one cloud in the vec, or else its an error"),
            hash_map,
        )
    }

    pub fn get_player_spawn_position(
        world: &mut World,
        config: &GameConfig,
        physics: &PhysicsWorld,
        cloud_entity: hecs::Entity,
    ) -> Vec2 {
        let spawn_cloud_collider_handle = {
            let (_, spawn_cloud_collider_handle) = world
                .query_one_mut::<(&SpawnCloudTag, &ColliderHandle)>(cloud_entity)
                .expect("There should be some spawn cloud");
            spawn_cloud_collider_handle.clone()
        };

        let spawn_cloud = Self::get_game_rectangle_from_collider_handle(
            config,
            physics,
            &spawn_cloud_collider_handle,
        );

        Vec2::new(
            spawn_cloud.x,
            spawn_cloud.y + (spawn_cloud.height / 2.) + CLOUD_BUFFER,
        )
    }

    fn sleeping_body_position(config: &GameConfig, player_position: Vec2) -> Vec2 {
        player_position + Vec2::new(0., config.player_game_height + SLEEPING_BODY_BUFFER)
    }

    fn spawn_exit(
        world: &mut hecs::World,
        physics: &mut PhysicsWorld,
        config: &GameConfig,
        level: &Level,
        root_scene: &mut SceneNode2d,
        _texture_manager: &mut TextureManager,
        render_map: &mut HashMap<SceneNodeKey, SceneNode2d>,
    ) -> hecs::Entity {
        let (collider_handle, outline_node_key) = Self::spawn_static_physics_object(
            root_scene,
            physics,
            config,
            level.exit_position().x,
            level.exit_position().y,
            level.exit_width,
            level.exit_height,
            EXIT_GROUP,
            render_map,
        );
        // TODO: Remove color here since this is the 1 bit game jam
        render_map
            .get_mut(&outline_node_key)
            .expect("this should exist")
            .set_color(kiss3d::color::WHITE);
        let center_node_key = SceneNodeKey(Uuid::new_v4());
        // unit rectangle, we will change the size and position during the sync
        let center_rect = root_scene.add_rectangle(1.0, 1.0).set_color(BLACK);
        render_map.insert(center_node_key, center_rect);
        let exit_node_keys = ExitNodeKeys {
            center: center_node_key,
            outline: outline_node_key,
        };
        world.spawn((Exit {}, collider_handle, exit_node_keys))
    }

    fn spawn_player(
        world: &mut hecs::World,
        physics: &mut PhysicsWorld,
        config: &GameConfig,
        position: Vec2,
        root_scene: &mut SceneNode2d,
        texture_manager: &mut TextureManager,
        render_map: &mut HashMap<SceneNodeKey, SceneNode2d>,
    ) -> hecs::Entity {
        let pixel_to_physics_scale = config.pixel_to_physics_scale();
        let rigid_body = RigidBodyBuilder::dynamic()
            .translation(config.convert_vec2_pixel_to_physics(position))
            // add some drag so that we coast to a stop
            .linear_damping(config.player_linear_damping)
            // don't rotate
            .lock_rotations()
            .build();

        let half_extents_x = config.player_game_width * pixel_to_physics_scale / 2.;
        let half_extents_y = config.player_game_height * pixel_to_physics_scale / 2.;

        let collider = ColliderBuilder::cuboid(half_extents_x, half_extents_y)
            .collision_groups(InteractionGroups::new(
                PLAYER_GROUP,
                STATIC_PLATFORM_GROUP
                    | SPAWN_CLOUD_GROUP
                    | EXIT_GROUP
                    | WALL_GROUP
                    | MOVING_PLATFORM_GROUP
                    | SLEEPING_BODY_GROUP,
                InteractionTestMode::And,
            ))
            .friction(config.player_ground_friction)
            // make collision friction depend on player only
            .friction_combine_rule(CoefficientCombineRule::Min)
            .build();

        let (body_handle, collider_handle) = physics.insert(rigid_body, collider);

        let controller = KinematicCharacterController {
            snap_to_ground: None,
            ..Default::default()
        };

        // spawn sleeping body on top
        let sleeping_body_position = Self::sleeping_body_position(config, position);
        let sleeping_body = RigidBodyBuilder::dynamic()
            .translation(config.convert_vec2_pixel_to_physics(sleeping_body_position))
            .lock_rotations()
            .build();
        let sleeping_collider = ColliderBuilder::cuboid(half_extents_x, half_extents_y)
            .collision_groups(InteractionGroups::new(
                SLEEPING_BODY_GROUP,
                STATIC_PLATFORM_GROUP
                    | SPAWN_CLOUD_GROUP
                    // we specifically exclude the exit group so that our player can enter it.
                    | WALL_GROUP
                    | MOVING_PLATFORM_GROUP
                    | PLAYER_GROUP,
                InteractionTestMode::And,
            ))
            .density(0.1)
            .build();

        let (sleeping_body_handle, sleeping_collider_handle) =
            physics.insert(sleeping_body, sleeping_collider);

        let mut sleeping_body_node =
            root_scene.add_rectangle(config.player_game_height, config.player_game_width);
        sleeping_body_node.set_rotation(PI / 2.);
        sleeping_body_node.set_texture(texture_manager.get("sleeper").unwrap());
        let sleeping_body_node_key = SceneNodeKey(Uuid::new_v4());
        render_map.insert(sleeping_body_node_key, sleeping_body_node);
        let sleeping_body = SleepingBody {
            body: sleeping_body_handle,
            collider: sleeping_collider_handle,
            node: sleeping_body_node_key,
        };

        let center = root_scene.add_rectangle(1.0, 1.0);
        let center_key = SceneNodeKey(Uuid::new_v4());
        render_map.insert(center_key, center);
        let left_truster = root_scene
            .add_rectangle(1., 1.)
            .set_texture(texture_manager.get("jet").unwrap());
        let left_thruster_key = SceneNodeKey(Uuid::new_v4());
        render_map.insert(left_thruster_key, left_truster);
        let right_truster = root_scene
            .add_rectangle(1., 1.)
            .set_texture(texture_manager.get("jet").unwrap());
        let right_thruster_key = SceneNodeKey(Uuid::new_v4());
        render_map.insert(right_thruster_key, right_truster);
        let bottom_truster = root_scene
            .add_rectangle(1., 1.)
            .set_texture(texture_manager.get("jet").unwrap());
        let bottom_thruster_key = SceneNodeKey(Uuid::new_v4());
        render_map.insert(bottom_thruster_key, bottom_truster);
        let player_node_keys = PlayerNodeKeys {
            center: center_key,
            left_thruster: left_thruster_key,
            right_thruster: right_thruster_key,
            bottom_thruster: bottom_thruster_key,
        };
        world.spawn((
            body_handle,
            collider_handle,
            controller,
            player_node_keys,
            sleeping_body,
        ))
    }

    pub fn check_object_at_world_position(&mut self, world_position: Vec2) {
        let query_pipeline = self.physics.query_pipeline();

        // first intersection that is either a spawn cloud or static platform wins
        let selected: Option<ColliderHandle> = (|| {
            for (collider_handle, collider) in query_pipeline
                .intersect_point(self.config.convert_vec2_pixel_to_physics(world_position))
            {
                if collider.collision_groups().memberships.contains(EXIT_GROUP) {
                    log!("Clicked on exit!");
                    return Some(collider_handle);
                } else if let Some(_spawn_cloud) = self.cloud_map.get(&collider_handle) {
                    log!("Clicked on a spawn cloud!");
                    return Some(collider_handle);
                } else if let Some(_static_platform) =
                    self.static_platform_map.get(&collider_handle)
                {
                    log!("Clicked on a static platform");
                    return Some(collider_handle);
                } else if let Some(_moving_platform) =
                    self.moving_platform_map.get(&collider_handle)
                {
                    log!("Clicked on a moving platform (point 1)");
                    return Some(collider_handle);
                } else if let Some(_endpoint) =
                    self.moving_platform_endpoint_map.get(&collider_handle)
                {
                    log!("Clicked on a moving platform's point 2");
                    return Some(collider_handle);
                }
            }
            None
        })();

        if selected.is_none() {
            log!("Clicked on nothing!");
        }

        self.selected_collider = selected;
    }

    fn get_game_rectangle_from_collider_handle(
        config: &GameConfig,
        physics: &PhysicsWorld,
        collider_handle: &ColliderHandle,
    ) -> GameRectangle {
        let collider = &physics.colliders[*collider_handle];

        let position = config.convert_vec2_physics_to_pixel(collider.position().translation);

        let half_extents = collider
            .shape()
            .as_cuboid()
            .expect("every physics object here should be a cuboid")
            .half_extents;

        GameRectangle {
            x: position.x,
            y: position.y,
            width: half_extents.x * 2.0 * config.physics_to_pixel_scale,
            height: half_extents.y * 2.0 * config.physics_to_pixel_scale,
        }
    }

    pub fn set_current_selected_as_first_spawn(&mut self) {
        if self.edit_mode != EditMode::None {
            if let Some(collider_handle) = self.selected_collider
                && let Some(cloud_entity) = self.cloud_map.get(&collider_handle)
            {
                log!("Set first spawn cloud");
                self.starting_cloud = *cloud_entity;
            } else {
                log!("Spawn cloud not selected");
            }
        } else {
            log!("Can't do this, not in edit mode");
        }
    }

    /// Detaches the entity's render node and removes it from the ECS world.
    /// The entity's collider is left alone, that's the caller's job.
    fn despawn_entity(&mut self, entity: Entity) {
        let render_node_key = *self
            .world
            .query_one_mut::<&SceneNodeKey>(entity)
            .expect("All editable objects should have a render node key");
        let render_node = self
            .render_map
            .get_mut(&render_node_key)
            .expect("should have a render node from key");
        render_node.detach();
        let _ = self.world.despawn(entity);
    }

    fn remove_selected(&mut self) -> Option<(EditableObjectType, GameRectangle)> {
        let selected_handle = self.selected_collider?;
        // we cannot remove the exit entity
        if selected_handle == self.get_exit_collider_handle() {
            log!("Can't remove the exit");
            return None;
        }
        // grabbing a platform by its point 2 handle still means the whole platform
        let selected_handle = self.resolve_selection_to_platform(selected_handle);

        // a moving platform's collider hangs off a body that has to be torn down too, but
        // only once the collider itself is gone
        let mut platform_body = None;
        let object_type = if let Some(entity) = self.static_platform_map.remove(&selected_handle) {
            self.despawn_entity(entity);
            EditableObjectType::StaticPlatform
        } else if let Some(entity) = self.cloud_map.remove(&selected_handle) {
            if self.cloud_map.is_empty() {
                log!("Sorry! I can't let you do that. We have to have at least one spawn cloud.");
                self.cloud_map.insert(selected_handle, entity);
                return None;
            }
            self.despawn_entity(entity);
            // if entity was the starting cloud, we have to replace it
            if entity == self.starting_cloud {
                let (_, replacement_cloud_entity) = self
                    .cloud_map
                    .iter()
                    .next()
                    .expect("Should be a cloud map entry here");
                self.starting_cloud = *replacement_cloud_entity;
            }
            if entity == self.current_spawn_cloud {
                // the starting cloud is guaranteed to still exist at this point
                self.current_spawn_cloud = self.starting_cloud;
            }
            EditableObjectType::SpawnCloud
        } else if let Some(entity) = self.moving_platform_map.remove(&selected_handle) {
            let path = *self
                .world
                .query_one_mut::<&MovementPath>(entity)
                .expect("a moving platform always has a patrol path");
            // the point 2 handle is part of the platform, so it goes down with it
            self.moving_platform_endpoint_map
                .remove(&path.endpoint_collider);
            self.physics.remove_collider(path.endpoint_collider);
            self.despawn_entity(entity);
            platform_body = Some(path.body);
            EditableObjectType::MovingPlatform
        } else {
            log!("This selected collider doesn't exist! Uh oh, that's bade");
            unreachable!(
                "If we somehow got here, and the collider handle doesn't exist, that's very bad"
            );
        };

        // grab the rectangle before the collider goes away
        let rectangle = Self::get_game_rectangle_from_collider_handle(
            &self.config,
            &self.physics,
            &selected_handle,
        );
        self.physics.remove_collider(selected_handle);
        if let Some(body) = platform_body {
            self.physics.remove_body(body);
        }
        self.selected_collider = None;

        Some((object_type, rectangle))
    }

    pub fn delete_selected(&mut self) {
        if self.edit_mode == EditMode::None {
            log!("Can't do this, not in edit mode");
            return;
        }
        if self.selected_collider.is_none() {
            log!("Nothing selected to delete");
            return;
        }
        if let Some((object_type, _)) = self.remove_selected() {
            log!("deleted selected {:?}", object_type);
        }
    }

    pub fn create_or_change_type_of_selected(
        &mut self,
        root_scene: &mut SceneNode2d,
        texture_manager: &mut TextureManager,
        position: Vec2,
    ) {
        if self.edit_mode == EditMode::None {
            log!("Can't do this, not in edit mode");
            return;
        }

        if self.selected_collider.is_none() {
            log!("create static platform by default");
            let static_platform = StaticPlatform {
                position: (position.x, position.y),
                width: 50.,
                height: 10.,
            };
            let (collider_handle, entity) = Self::spawn_static_platform(
                &mut self.world,
                &mut self.physics,
                &self.config,
                &static_platform,
                root_scene,
                texture_manager,
                &mut self.render_map,
            );
            self.static_platform_map.insert(collider_handle, entity);
            self.selected_collider = Some(collider_handle);
            return;
        }

        // swap the selected object for one of the other type, in the same place
        let Some((object_type, rectangle)) = self.remove_selected() else {
            return;
        };
        let collider_handle = match object_type {
            EditableObjectType::MovingPlatform => {
                log!("swapping moving platform for static platform");
                let static_platform = StaticPlatform {
                    position: (rectangle.x, rectangle.y),
                    width: rectangle.width,
                    height: rectangle.height,
                };
                let (collider_handle, entity) = Self::spawn_static_platform(
                    &mut self.world,
                    &mut self.physics,
                    &self.config,
                    &static_platform,
                    root_scene,
                    texture_manager,
                    &mut self.render_map,
                );
                self.static_platform_map.insert(collider_handle, entity);
                collider_handle
            }
            EditableObjectType::StaticPlatform => {
                log!("swapping static platform for spawn cloud");
                let spawn_cloud = SpawnCloud {
                    position: (rectangle.x, rectangle.y),
                    width: rectangle.width,
                    height: rectangle.height,
                };
                let (collider_handle, entity) = Self::spawn_spawn_cloud(
                    &mut self.world,
                    &mut self.physics,
                    &self.config,
                    &spawn_cloud,
                    root_scene,
                    texture_manager,
                    &mut self.render_map,
                );
                self.cloud_map.insert(collider_handle, entity);
                collider_handle
            }
            EditableObjectType::SpawnCloud => {
                log!("swapping spawn_cloud for moving_platform");
                let point_1 = rectangle.get_position();
                let point_2 = point_1 + DEFAULT_ENDPOINT_OFFSET;
                let moving_platform = MovingPlatform {
                    point_1: (point_1.x, point_1.y),
                    point_2: (point_2.x, point_2.y),
                    width: rectangle.width,
                    height: rectangle.height,
                    speed: DEFAULT_MOVING_PLATFORM_SPEED,
                };
                let (collider_handle, endpoint_collider, entity) = Self::spawn_moving_platform(
                    &mut self.world,
                    &mut self.physics,
                    &self.config,
                    &moving_platform,
                    root_scene,
                    texture_manager,
                    &mut self.render_map,
                );
                self.moving_platform_map.insert(collider_handle, entity);
                self.moving_platform_endpoint_map
                    .insert(endpoint_collider, entity);
                collider_handle
            }
        };
        self.selected_collider = Some(collider_handle);
    }

    pub fn get_spawn_clouds(&mut self) -> Vec<GameRectangle> {
        let mut collider_handle_vec = Vec::new();
        {
            for (entity, collider_handle, _) in self
                .world
                .query_mut::<(Entity, &ColliderHandle, &SpawnCloudTag)>()
            {
                // make sure the starting cloud is at the start of the vec
                if self.starting_cloud == entity {
                    collider_handle_vec.insert(0, *collider_handle);
                } else {
                    collider_handle_vec.push(*collider_handle);
                }
            }
        }
        let mut platform_data_vec = Vec::new();
        for collider_handle in collider_handle_vec {
            platform_data_vec.push(Self::get_game_rectangle_from_collider_handle(
                &self.config,
                &self.physics,
                &collider_handle,
            ));
        }
        platform_data_vec
    }

    pub fn get_static_platforms(&mut self) -> Vec<GameRectangle> {
        let mut collider_handle_vec = Vec::new();
        {
            for (collider_handle, _) in self
                .world
                .query_mut::<(&ColliderHandle, &StaticPlatformTag)>()
            {
                collider_handle_vec.push(*collider_handle);
            }
        }
        let mut platform_data_vec = Vec::new();
        for collider_handle in collider_handle_vec {
            platform_data_vec.push(Self::get_game_rectangle_from_collider_handle(
                &self.config,
                &self.physics,
                &collider_handle,
            ));
        }
        platform_data_vec
    }

    pub fn get_moving_platforms(&mut self) -> Vec<MovingPlatform> {
        let mut patrol_vec = Vec::new();
        {
            for (_, path, collider_handle) in
                self.world
                    .query_mut::<(&MovingPlatformTag, &MovementPath, &ColliderHandle)>()
            {
                patrol_vec.push((*path, *collider_handle));
            }
        }
        patrol_vec
            .into_iter()
            .map(|(path, collider_handle)| {
                let rectangle = Self::get_game_rectangle_from_collider_handle(
                    &self.config,
                    &self.physics,
                    &collider_handle,
                );
                MovingPlatform {
                    point_1: (path.point_1.x, path.point_1.y),
                    point_2: (path.point_2.x, path.point_2.y),
                    width: rectangle.width,
                    height: rectangle.height,
                    speed: path.speed,
                }
            })
            .collect()
    }

    pub fn get_all_endpoints(&self) -> Vec<GameRectangle> {
        self.moving_platform_endpoint_map
            .keys()
            .map(|collider_handle| {
                Self::get_game_rectangle_from_collider_handle(
                    &self.config,
                    &self.physics,
                    collider_handle,
                )
            })
            .collect()
    }

    /// Every moving platform's two patrol points, in game (pixel) space, so the renderer
    /// can draw the path each one travels along.
    pub fn get_moving_platform_paths(&mut self) -> Vec<(Vec2, Vec2)> {
        self.world
            .query_mut::<(&MovingPlatformTag, &MovementPath)>()
            .into_iter()
            .map(|(_, path)| (path.point_1, path.point_2))
            .collect()
    }

    pub fn get_walls(&mut self) -> Vec<GameRectangle> {
        let mut collider_handle_vec = Vec::new();
        {
            for (collider_handle, _) in self.world.query_mut::<(&ColliderHandle, &Wall)>() {
                collider_handle_vec.push(*collider_handle);
            }
        }
        let mut platform_data_vec = Vec::new();
        for collider_handle in collider_handle_vec {
            platform_data_vec.push(Self::get_game_rectangle_from_collider_handle(
                &self.config,
                &self.physics,
                &collider_handle,
            ));
        }
        platform_data_vec
    }

    fn get_player_physics_data(&mut self) -> (&RigidBodyHandle, &KinematicCharacterController) {
        self.world
            .query_one_mut::<(&RigidBodyHandle, &KinematicCharacterController)>(self.player)
            .expect("Player should exist here")
    }

    // returns position in game space not physics space
    pub fn update_position_with_input(&mut self, input: &MoveInputState) {
        let (handle, _) = self.get_player_physics_data();
        let handle = *handle;
        // following code is based off of:
        // https://github.com/dimforge/rapier/blob/c13133ad293ee70c7f9cec9e498eac016c362169/examples2d/utils/character.rs#L125

        let pixel_to_physics_scale = self.config.pixel_to_physics_scale();
        let horizontal_acceleration = self.config.player_horizontal_acceleration;
        let vertical_acceleration = self.config.player_vertical_acceleration;

        // Scaling a point 2 handle scales the platform it belongs to instead: the handle's
        // size is only ever a mirror of the platform's, so scaling it directly would just
        // get overwritten on the next tick.
        let scale_target = self
            .selected_collider
            .map(|collider_handle| self.resolve_selection_to_platform(collider_handle));
        // A moving platform is positioned by its body, so nudging its collider directly
        // would be undone by the next step. Everything else is a loose collider.
        let move_target_body = self
            .selected_collider
            .and_then(|collider_handle| self.moving_platform_body(collider_handle));

        let character_body = &mut self.physics.bodies[handle];

        match self.edit_mode {
            EditMode::Move => {
                character_body.set_enabled(false);
                if let Some(handle) = self.selected_collider {
                    let nudge = Vec2::new(
                        ((input.right as i8) - (input.left as i8)) as f32,
                        ((input.up as i8) - (input.down as i8)) as f32,
                    ) * GAME_TIME_DELTA;
                    if let Some(body_handle) = move_target_body {
                        let body = &mut self.physics.bodies[body_handle];
                        let translation = body.position().translation + nudge;
                        // teleport, so dragging in the editor never builds up a velocity
                        body.set_position(Pose2::from_translation(translation), true);
                    } else {
                        let collider = &mut self.physics.colliders[handle];
                        let translation = collider.position().translation + nudge;
                        collider.set_position(Pose2::from_translation(translation));
                    }
                }
            }
            EditMode::Scale => {
                character_body.set_enabled(false);
                if let Some(handle) = scale_target {
                    let collider = &mut self.physics.colliders[handle];
                    let cuboid = collider
                        .shape_mut()
                        .as_cuboid_mut()
                        .expect("every physics object here should be a cuboid");
                    cuboid.half_extents += Vec2::new(
                        ((input.right as i8) - (input.left as i8)) as f32,
                        ((input.up as i8) - (input.down as i8)) as f32,
                    ) * GAME_TIME_DELTA;
                }
            }
            EditMode::None => {
                character_body.set_enabled(true);
                let acceleration = Vec2::new(
                    ((input.right as i8) - (input.left as i8)) as f32 * horizontal_acceleration,
                    (input.thrust as i8) as f32 * vertical_acceleration,
                ) * pixel_to_physics_scale;

                let mass = character_body.mass();
                // we want a consistent force, not adding force over time
                character_body.reset_forces(false);
                // we want a smooth (ish) speeding up and slowing down
                // rapier automatically integrates the delta for us (since we set it earlier)
                character_body.add_force(acceleration * mass, true);
                self.current_player_input = *input;
            }
        }
    }

    pub fn set_player_position(&mut self, position: Vec2) {
        let (handle, _) = self.get_player_physics_data();
        let handle = *handle;
        let character_body = &mut self.physics.bodies[handle];
        let physics_position = self.config.convert_vec2_pixel_to_physics(position);
        let physics_position = Pose2::from_translation(physics_position);
        // remove all forces
        character_body.reset_forces(true);
        character_body.set_position(physics_position, true);

        // spawn sleeping body on top of the player
        let sleeping_body_handle = self.get_sleeping_body().body;
        let sleeping_body_position = self
            .config
            .convert_vec2_pixel_to_physics(Self::sleeping_body_position(&self.config, position));
        let sleeping_body = &mut self.physics.bodies[sleeping_body_handle];
        sleeping_body.reset_forces(true);
        // whatever speed it picked up on the way down doesn't carry over into the new life
        sleeping_body.set_linvel(Vec2::ZERO, true);
        sleeping_body.set_position(Pose2::from_translation(sleeping_body_position), true);
    }

    fn get_sleeping_body(&mut self) -> &SleepingBody {
        self.world
            .query_one_mut::<&SleepingBody>(self.player)
            .expect("the player always carries a sleeping body")
    }

    fn sleeping_body_hit_something(&mut self) -> bool {
        let sleeping_collider = self.get_sleeping_body().collider;
        let player_collider = *self
            .world
            .query_one_mut::<&ColliderHandle>(self.player)
            .expect("Player should exist here");

        self.physics
            .contact_pairs_with(sleeping_collider)
            // a pair exists as soon as the two are near each other; only solver contacts
            // mean they're actually touching
            .filter(|pair| pair.has_any_active_contact())
            .any(|pair| {
                let other = if pair.collider1 == sleeping_collider {
                    pair.collider2
                } else {
                    pair.collider1
                };
                other != player_collider
            })
    }

    /// Ignore collisions that shouldn't cause the game to restart the player
    fn player_query_groups(collider: &Collider) -> InteractionGroups {
        let groups = collider.collision_groups();
        groups.with_filter(groups.filter.difference(SLEEPING_BODY_GROUP))
    }

    /// How far below the player we still count a platform as being ridden, in pixels.
    /// Only has to cover the gap a descending platform can open up in a single tick.
    const RIDE_PROBE_DISTANCE: f32 = 1.0;

    fn ridden_platform_fall_speed(&self, player_body: RigidBodyHandle) -> Option<Vec2> {
        let body = &self.physics.bodies[player_body];
        let collider = &self.physics.colliders[body.colliders()[0]];

        let (hit_handle, _) = self.physics.cast_shape(
            collider.position(),
            -Vec2::Y,
            collider.shape(),
            ShapeCastOptions {
                target_distance: 0.,
                stop_at_penetration: true,
                max_time_of_impact: Self::RIDE_PROBE_DISTANCE
                    * self.config.pixel_to_physics_scale(),
                compute_impact_geometry_on_penetration: true,
            },
            QueryFilter::new()
                .exclude_rigid_body(player_body)
                .groups(Self::player_query_groups(collider)),
        )?;

        // only moving platforms carry anyone; everything else underfoot holds still
        let platform_body = self
            .moving_platform_map
            .contains_key(&hit_handle)
            .then(|| {
                self.physics.colliders[hit_handle]
                    .parent()
                    .expect("a moving platform's collider always hangs off its body")
            })?;
        Some(self.physics.bodies[platform_body].linvel())
    }

    /// How deep the player is currently buried in solid level geometry, in pixels.
    fn player_overlap_depth(&mut self) -> f32 {
        let body_handle = *self.get_player_physics_data().0;
        let player_collider = self.physics.bodies[body_handle].colliders()[0];
        let crushing_groups = STATIC_PLATFORM_GROUP | MOVING_PLATFORM_GROUP | WALL_GROUP;

        let mut deepest = 0.;
        for pair in self.physics.contact_pairs_with(player_collider) {
            let other = if pair.collider1 == player_collider {
                pair.collider2
            } else {
                pair.collider1
            };
            if !self.physics.colliders[other]
                .collision_groups()
                .memberships
                .intersects(crushing_groups)
            {
                continue;
            }
            // dist goes negative once the two shapes actually interpenetrate; while
            // they're merely close it stays positive and we don't care
            if let Some((_, contact)) = pair.find_deepest_contact()
                && contact.dist < 0.
            {
                deepest = f32::max(deepest, -contact.dist);
            }
        }
        deepest * self.config.physics_to_pixel_scale
    }

    pub fn check_player_collisions(&mut self) -> CollisionType {
        let (handle, controller) = self.get_player_physics_data();
        let handle = *handle;
        let controller = *controller;
        // following code is based off of:
        // https://github.com/dimforge/rapier/blob/c13133ad293ee70c7f9cec9e498eac016c362169/examples2d/utils/character.rs#L125

        let character_body = &self.physics.bodies[handle];
        let character_collider = &self.physics.colliders[character_body.colliders()[0]];
        let character_rotation = character_collider.position().rotation;
        let character_shape = character_collider.shared_shape().clone();
        let character_mass = character_body.mass();
        // this is basically like physics masks in other game engines
        // so who can collide with the character collider, and what the character collider can collide against
        // (those may not be the same thing.)
        let character_groups = Self::player_query_groups(character_collider);

        let current_position = character_body.position().translation;
        // update_position_with_input already set linvel from raw input; sweep by
        // how far that velocity would move the body this tick.
        let desired_movement = character_body.linvel() * GAME_TIME_DELTA;
        // Player's speed in pixel space (px/s), for comparing against the
        // config's crash threshold, which is expressed in the same units.
        let player_speed = character_body.linvel() * self.config.physics_to_pixel_scale;
        // check all collisions in the physics world against this character body
        let mut query_pipeline = self.physics.broad_phase.as_query_pipeline_mut(
            self.physics.narrow_phase.query_dispatcher(),
            &mut self.physics.bodies,
            &mut self.physics.colliders,
            QueryFilter::new()
                .exclude_rigid_body(handle)
                .groups(character_groups),
        );

        let mut collisions = vec![];
        let movement = controller.move_shape(
            GAME_TIME_DELTA,
            &query_pipeline.as_ref(),
            &*character_shape,
            &Pose2 {
                rotation: character_rotation,
                translation: current_position,
            },
            desired_movement,
            |c| collisions.push(c),
        );

        controller.solve_character_collision_impulses(
            GAME_TIME_DELTA,
            &mut query_pipeline,
            &*character_shape,
            character_mass,
            &collisions,
        );

        // figure out what type of collision has happened
        let mut collision_type = CollisionType::None;
        for collision in &collisions {
            let hit_groups = self.physics.colliders[collision.handle].collision_groups();
            // as soon as we exit, we exit the level
            if hit_groups.memberships.contains(EXIT_GROUP) {
                collision_type = CollisionType::Exit;
                break;
            }
            // the spawn cloud is meant to be landed on, never crashed into
            if hit_groups.memberships.contains(SPAWN_CLOUD_GROUP) {
                let cloud_entity = self
                    .cloud_map
                    .get(&collision.handle)
                    .expect("This should be a cloud entity");
                self.current_spawn_cloud = *cloud_entity;
                continue;
            }

            let normal = collision.hit.normal1;
            if normal.x.abs() > normal.y.abs()
                && player_speed.x.abs() > self.config.player_speed_for_horizontal_crash
            {
                log!("Player has crashed horizontally!");
                collision_type = CollisionType::Horizontal;
            } else if player_speed.y.abs() > self.config.player_speed_for_vertical_crash {
                log!("Player has crashed vertically!");
                collision_type = CollisionType::Vertical;
            }
            // we only care about the first thing we're colliding with
            break;
        }

        // check if we got crushed, since that doesn't technically count as a collision
        if collision_type == CollisionType::None {
            let overlap = self.player_overlap_depth();
            if overlap > self.config.player_overlap_for_crash {
                log!("Player has been crushed! {overlap:.2}px into something solid");
                collision_type = CollisionType::Crushed;
            }
        }

        // Replace the raw input velocity with the wall-corrected one so the
        // upcoming physics.step() moves the body by exactly the safe amount.
        let pixel_to_physics_scale = self.config.pixel_to_physics_scale();
        let max_horizontal_speed = self.config.player_max_horizontal_speed;
        let max_vertical_speed = self.config.player_max_vertical_speed;
        let mut safe_velocity = movement.translation / GAME_TIME_DELTA;
        safe_velocity.x = safe_velocity.x.clamp(
            -max_horizontal_speed * pixel_to_physics_scale,
            max_horizontal_speed * pixel_to_physics_scale,
        );
        safe_velocity.y = safe_velocity.y.clamp(
            -max_vertical_speed * pixel_to_physics_scale,
            max_vertical_speed * pixel_to_physics_scale,
        );
        // make sure we get carried by a platform that is moving down and we aren't moving
        if let Some(fall_speed) = self.ridden_platform_fall_speed(handle)
            && fall_speed.y < safe_velocity.y
            && !self.current_player_input.thrust
        {
            safe_velocity = fall_speed;
        }
        self.physics.bodies[handle].set_linvel(safe_velocity, true);

        collision_type
    }

    /// Returns position in game (pixel) space, not physics space.
    pub fn get_player_position_and_velocity(&mut self) -> (Vec2, Vec2) {
        let (handle, _) = self.get_player_physics_data();
        let handle = handle.clone();
        let physics_to_pixel_scale = self.config.physics_to_pixel_scale;
        let body = &mut self.physics.bodies[handle];
        (
            body.position().translation * physics_to_pixel_scale,
            body.linvel() * physics_to_pixel_scale,
        )
    }

    pub fn get_current_level(&self) -> &Level {
        &self.current_level
    }

    fn get_exit_collider_handle(&mut self) -> ColliderHandle {
        self.world
            .query_one_mut::<&ColliderHandle>(self.exit_entity)
            .expect("There should have been a collider assigned to the exit when it was created")
            .clone()
    }

    pub fn get_exit_rectangle(&mut self) -> GameRectangle {
        let collider_handle = self.get_exit_collider_handle();
        Self::get_game_rectangle_from_collider_handle(&self.config, &self.physics, &collider_handle)
    }

    pub fn step(&mut self) -> GameEvents {
        // run the collisions checks first before doing physics step
        // otherwise this causes player to clip through walls
        let collision_type = self.check_player_collisions();
        // hitting the sleeping body on something is a failure state
        let sleeping_body_hit_something =
            self.edit_mode == EditMode::None && self.sleeping_body_hit_something();
        if sleeping_body_hit_something {
            log!("The sleeping body has hit something!");
        }
        let mut game_event = GameEvents::None;
        if collision_type == CollisionType::Exit {
            game_event = GameEvents::Exit;
        } else if collision_type != CollisionType::None || sleeping_body_hit_something {
            let new_position = Self::get_player_spawn_position(
                &mut self.world,
                &self.config,
                &self.physics,
                self.current_spawn_cloud,
            );
            self.set_player_position(new_position);
            self.park_moving_platforms();
            game_event = GameEvents::Respawn;
        }
        // Move the platforms right before the step so the broad phase gets rebuilt around
        // their new positions; the next tick's collision check then queries an index that
        // agrees with where the colliders actually are.
        self.update_moving_platforms();
        self.physics.step();
        Self::sync_scene_nodes(
            &self.config,
            &mut self.world,
            &self.physics,
            &mut self.render_map,
            &self.current_player_input,
        );

        game_event
    }

    pub fn create_level_from_game_logic(&mut self) -> Level {
        let mut trapped_box = GameRectangle::new(
            0.,
            0.,
            self.current_level.player_trapped_box_width,
            self.current_level.player_trapped_box_height,
        );
        let mut wall_width = self.current_level.player_trapped_box_wall_width;
        for rectangle in self.get_walls() {
            if rectangle.x.abs() > rectangle.y.abs() {
                trapped_box.height = rectangle.height;
                wall_width = rectangle.width;
            } else {
                trapped_box.width = rectangle.width;
                wall_width = rectangle.height;
            }
        }

        let spawn_clouds: Vec<SpawnCloud> = self
            .get_spawn_clouds()
            .iter()
            .map(|rectangle| SpawnCloud {
                position: (rectangle.x, rectangle.y),
                width: rectangle.width,
                height: rectangle.height,
            })
            .collect();

        let static_platforms: Vec<crate::level::StaticPlatform> = self
            .get_static_platforms()
            .iter()
            .map(|rectangle| crate::level::StaticPlatform {
                position: (rectangle.x, rectangle.y),
                width: rectangle.width,
                height: rectangle.height,
            })
            .collect();

        let moving_platforms = self.get_moving_platforms();

        let exit_rectangle = self.get_exit_rectangle();

        Level::new(
            trapped_box,
            wall_width,
            spawn_clouds,
            static_platforms,
            moving_platforms,
            exit_rectangle,
        )
    }
}
