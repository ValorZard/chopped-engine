use anyhow::{Context, Result};
use chopped_asset_handler::fetch_asset_bytes;
use chopped_engine::{kiss3d::glamx::Vec2, util::GameRectangle};

#[derive(serde::Serialize, serde::Deserialize, Clone, PartialEq, PartialOrd)]
pub struct SpawnCloud {
    pub position: (f32, f32),
    pub width: f32,
    pub height: f32,
}

impl SpawnCloud {
    pub fn get_vec2_position(&self) -> Vec2 {
        Vec2::new(self.position.0, self.position.1)
    }
}

#[derive(serde::Serialize, serde::Deserialize, PartialEq, PartialOrd)]
pub struct StaticPlatform {
    pub position: (f32, f32),
    pub width: f32,
    pub height: f32,
}

impl StaticPlatform {
    pub fn get_vec2_position(&self) -> Vec2 {
        Vec2::new(self.position.0, self.position.1)
    }
}

/// A platform that move back and forth between two points forever.
#[derive(serde::Serialize, serde::Deserialize, Clone, PartialEq, PartialOrd)]
pub struct MovingPlatform {
    /// Where the platform sits when the level starts, and the end it keeps returning to.
    pub point_1: (f32, f32),
    /// The final end of the platform path
    pub point_2: (f32, f32),
    pub width: f32,
    pub height: f32,
    /// How fast the platform travels along its path, in pixels per second.
    pub speed: f32,
}

impl MovingPlatform {
    pub fn get_vec2_point_1(&self) -> Vec2 {
        Vec2::new(self.point_1.0, self.point_1.1)
    }

    pub fn get_vec2_point_2(&self) -> Vec2 {
        Vec2::new(self.point_2.0, self.point_2.1)
    }
}

/// Tunable gameplay constants, loaded from `assets/config.ron`.
#[derive(serde::Serialize, serde::Deserialize, PartialEq)]
pub struct Level {
    pub player_trapped_box_width: f32,
    pub player_trapped_box_height: f32,
    pub player_trapped_box_wall_width: f32,

    // there has to be at least one spawn cloud
    // the first spawn cloud at index 0 is the cloud that we start the level on
    pub spawn_clouds: Vec<SpawnCloud>,
    pub static_platforms: Vec<StaticPlatform>,
    /// Defaulted so that levels authored before moving platforms existed still load.
    #[serde(default)]
    pub moving_platforms: Vec<MovingPlatform>,

    /// Where the spawn cloud sits in game (pixel) space; player spawns on top
    exit_position: (f32, f32),
    pub exit_width: f32,
    pub exit_height: f32,
}

impl Level {
    /// Builds a level out of current game logic
    pub fn new(
        trapped_box: GameRectangle,
        player_trapped_box_wall_width: f32,
        spawn_clouds: Vec<SpawnCloud>,
        static_platforms: Vec<StaticPlatform>,
        moving_platforms: Vec<MovingPlatform>,
        exit_rectangle: GameRectangle,
    ) -> Self {
        Self {
            player_trapped_box_width: trapped_box.width,
            player_trapped_box_height: trapped_box.height,
            player_trapped_box_wall_width,
            spawn_clouds,
            static_platforms,
            moving_platforms,
            exit_position: (exit_rectangle.x, exit_rectangle.y),
            exit_width: exit_rectangle.width,
            exit_height: exit_rectangle.height,
        }
    }

    pub async fn create_from_file(file_name: &str) -> Self {
        let bytes = fetch_asset_bytes(&format!("{file_name}.level"), env!("CARGO_MANIFEST_DIR"))
            .await
            .expect("should be able to fetch config.ron");
        ron::de::from_bytes(&bytes).expect("level file should be valid RON")
    }

    pub fn exit_position(&self) -> Vec2 {
        Vec2::new(self.exit_position.0, self.exit_position.1)
    }

    /// for now, this only works in a dev environment
    /// (I'd like to make this accessible to players though somehow)
    #[cfg(not(target_arch = "wasm32"))]
    pub fn write_level_to_disc(&self, file_name: &str) -> Result<()> {
        let contents = ron::ser::to_string_pretty(self, ron::ser::PrettyConfig::default())
            .context("failed to serialize level to RON")?;
        let assets_dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("assets");

        let mut path = assets_dir.join(format!("{file_name}.level"));
        let mut suffix = 1;
        while path.exists() {
            path = assets_dir.join(format!("{file_name}_{suffix}.level"));
            suffix += 1;
        }

        std::fs::write(&path, contents)
            .with_context(|| format!("failed to write level to {}", path.display()))
    }

    #[cfg(not(target_arch = "wasm32"))]
    pub fn read_direct_from_disc(file_name: &str) -> Self {
        let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("assets")
            .join(format!("{file_name}.level"));
        ron::de::from_bytes(&std::fs::read(&path).unwrap()).unwrap()
    }
}
