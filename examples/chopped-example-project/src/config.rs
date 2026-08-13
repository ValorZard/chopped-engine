use chopped_engine::kiss3d::glamx::Vec2;

pub const CONFIG_PATH: &str = "config.ron";

/// Tunable gameplay constants, loaded from `assets/config.ron`.
#[derive(serde::Deserialize)]
pub struct GameConfig {
    /// 1 meter in physics engine equals this many pixels.
    pub physics_to_pixel_scale: f32,

    pub player_game_width: f32,
    pub player_game_height: f32,

    /// all of the following variables use pixel to physics conversions internally,
    /// so we set all of these variables over in pixel space.
    /// our rapier based logic will automatically convert it to physics numbers.
    pub player_horizontal_acceleration: f32,
    pub player_vertical_acceleration: f32,
    /// this is basically the same thing as drag
    pub player_linear_damping: f32,
    pub player_ground_friction: f32,
    pub player_max_horizontal_speed: f32,
    pub player_max_vertical_speed: f32,
    pub player_speed_for_horizontal_crash: f32,
    pub player_speed_for_vertical_crash: f32,
    /// How far the player may sink into solid geometry before it counts as a crash, in
    /// pixels. Resting on something always overlaps it a little, so this can't be zero.
    pub player_overlap_for_crash: f32,
}

impl GameConfig {
    pub fn pixel_to_physics_scale(&self) -> f32 {
        1.0 / self.physics_to_pixel_scale
    }

    pub fn convert_vec2_pixel_to_physics(&self, position: Vec2) -> Vec2 {
        position * self.pixel_to_physics_scale()
    }

    pub fn convert_vec2_physics_to_pixel(&self, position: Vec2) -> Vec2 {
        position * self.physics_to_pixel_scale
    }

    #[cfg(not(target_arch = "wasm32"))]
    pub fn read_direct_from_disc() -> Self {
        let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("assets")
            .join(CONFIG_PATH);
        ron::de::from_bytes(&std::fs::read(&path).unwrap()).unwrap()
    }
}
